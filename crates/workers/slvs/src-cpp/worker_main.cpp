// SPDX-License-Identifier: GPL-3.0-or-later
//
// threeterm-slvs-worker: disposable worker binary for the ThreeTerm sketch solver.
// Reads a JSON sketch envelope from stdin, solves it through libslvs, and writes
// the normalized response JSON to stdout. Diagnostics go to stderr.
//
// This file is part of ThreeTerm; see ../NOTICE for upstream provenance.

#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <sstream>
#include <string>
#include <unordered_set>
#include <vector>

#include <slvs.h>

namespace {

constexpr const char* kSchemaVersion = "threeterm.workers.slvs/1";
constexpr uint32_t kGroup = 1;

std::string read_stdin() {
    std::string out;
    char buffer[4096];
    while (true) {
        std::size_t read = std::fread(buffer, 1, sizeof(buffer), stdin);
        if (read == 0) break;
        out.append(buffer, read);
        if (read < sizeof(buffer)) break;
    }
    return out;
}

void write_stdout_line(const std::string& line) {
    std::fwrite(line.data(), 1, line.size(), stdout);
    std::fputc('\n', stdout);
    std::fflush(stdout);
}

void write_stderr_line(const std::string& line) {
    std::fwrite(line.data(), 1, line.size(), stderr);
    std::fputc('\n', stderr);
    std::fflush(stderr);
}

// Minimal JSON parser tailored to the slice-1 envelope shape. Strict about
// the supported subset: objects, arrays, strings, numbers (decimal),
// booleans, and null. Returns an error string on the first violation.
class JsonParser {
public:
    enum class ValueKind { Object, Array, String, Number, Bool, Null };

    struct Value {
        ValueKind kind;
        std::string string_value;
        double number_value = 0.0;
        bool bool_value = false;
        std::vector<std::pair<std::string, Value>> object_value;
        std::vector<Value> array_value;
    };

    explicit JsonParser(const std::string& source) : source_(source), cursor_(0) {}

    bool parse_value(Value* out, std::string& error) {
        skip_ws();
        if (at_end()) { error = "unexpected end of input"; return false; }
        char c = peek();
        if (c == '{') return parse_object(out, error);
        if (c == '[') return parse_array(out, error);
        if (c == '"') return parse_string(out, error);
        if (c == 't' || c == 'f') return parse_bool(out, error);
        if (c == 'n') return parse_null(out, error);
        return parse_number(out, error);
    }

    bool parse_string(Value* out, std::string& error) {
        skip_ws();
        if (at_end() || source_[cursor_] != '"') {
            error = "expected string";
            return false;
        }
        cursor_++;
        std::string buffer;
        while (!at_end()) {
            char c = source_[cursor_++];
            if (c == '"') {
                out->kind = ValueKind::String;
                out->string_value = std::move(buffer);
                return true;
            }
            if (c == '\\') {
                if (at_end()) { error = "unterminated string escape"; return false; }
                char esc = source_[cursor_++];
                switch (esc) {
                    case '"': buffer.push_back('"'); break;
                    case '\\': buffer.push_back('\\'); break;
                    case '/': buffer.push_back('/'); break;
                    case 'b': buffer.push_back('\b'); break;
                    case 'f': buffer.push_back('\f'); break;
                    case 'n': buffer.push_back('\n'); break;
                    case 'r': buffer.push_back('\r'); break;
                    case 't': buffer.push_back('\t'); break;
                    case 'u':
                        if (cursor_ + 4 > source_.size()) {
                            error = "short unicode escape";
                            return false;
                        }
                        buffer.append(source_, cursor_, 4);
                        cursor_ += 4;
                        break;
                    default:
                        error = "unsupported string escape";
                        return false;
                }
            } else {
                buffer.push_back(c);
            }
        }
        error = "unterminated string";
        return false;
    }

    bool parse_number(Value* out, std::string& error) {
        skip_ws();
        std::size_t start = cursor_;
        if (!at_end() && (peek() == '-' || peek() == '+')) cursor_++;
        bool seen_digit = false;
        while (!at_end()) {
            char c = peek();
            if (c >= '0' && c <= '9') { cursor_++; seen_digit = true; continue; }
            break;
        }
        if (!at_end() && peek() == '.') {
            cursor_++;
            while (!at_end()) {
                char c = peek();
                if (c >= '0' && c <= '9') { cursor_++; seen_digit = true; continue; }
                break;
            }
        }
        if (!at_end() && (peek() == 'e' || peek() == 'E')) {
            cursor_++;
            if (!at_end() && (peek() == '+' || peek() == '-')) cursor_++;
            bool exp_digit = false;
            while (!at_end()) {
                char c = peek();
                if (c >= '0' && c <= '9') { cursor_++; exp_digit = true; continue; }
                break;
            }
            if (!exp_digit) { error = "malformed exponent"; return false; }
        }
        if (!seen_digit) { error = "expected number"; return false; }
        try {
            out->kind = ValueKind::Number;
            out->number_value = std::stod(source_.substr(start, cursor_ - start));
            return true;
        } catch (...) {
            error = "number out of range";
            return false;
        }
    }

    bool parse_bool(Value* out, std::string& error) {
        skip_ws();
        if (match("true")) { out->kind = ValueKind::Bool; out->bool_value = true; return true; }
        if (match("false")) { out->kind = ValueKind::Bool; out->bool_value = false; return true; }
        error = "expected boolean";
        return false;
    }

    bool parse_null(Value* out, std::string& error) {
        skip_ws();
        if (match("null")) { out->kind = ValueKind::Null; return true; }
        error = "expected null";
        return false;
    }

    bool consume(char ch, std::string& error) {
        skip_ws();
        if (at_end() || source_[cursor_] != ch) {
            error = std::string("expected '") + ch + "'";
            return false;
        }
        cursor_++;
        return true;
    }

    bool consume_or_end(char ch, std::string& error) {
        skip_ws();
        if (at_end()) { error = "unexpected end of input"; return false; }
        if (source_[cursor_] != ch) {
            error = std::string("expected '") + ch + "'";
            return false;
        }
        cursor_++;
        return true;
    }

    void skip_ws() {
        while (!at_end()) {
            char c = source_[cursor_];
            if (c == ' ' || c == '\t' || c == '\n' || c == '\r') cursor_++;
            else break;
        }
    }

    bool at_end() const { return cursor_ >= source_.size(); }

    char peek() const { return source_[cursor_]; }

    std::size_t cursor() const { return cursor_; }

private:

    bool parse_object(Value* out, std::string& error) {
        if (!consume('{', error)) return false;
        Value result;
        result.kind = ValueKind::Object;
        skip_ws();
        if (!at_end() && peek() == '}') { cursor_++; *out = std::move(result); return true; }
        while (true) {
            Value key_value;
            if (!parse_string(&key_value, error)) return false;
            if (!consume(':', error)) return false;
            Value child;
            if (!parse_value(&child, error)) return false;
            result.object_value.emplace_back(std::move(key_value.string_value), std::move(child));
            skip_ws();
            if (at_end()) { error = "unterminated object"; return false; }
            char c = peek();
            if (c == ',') { cursor_++; continue; }
            if (c == '}') { cursor_++; break; }
            error = "expected ',' or '}' in object";
            return false;
        }
        *out = std::move(result);
        return true;
    }

    bool parse_array(Value* out, std::string& error) {
        if (!consume('[', error)) return false;
        Value result;
        result.kind = ValueKind::Array;
        skip_ws();
        if (!at_end() && peek() == ']') { cursor_++; *out = std::move(result); return true; }
        while (true) {
            Value child;
            if (!parse_value(&child, error)) return false;
            result.array_value.push_back(std::move(child));
            skip_ws();
            if (at_end()) { error = "unterminated array"; return false; }
            char c = peek();
            if (c == ',') { cursor_++; continue; }
            if (c == ']') { cursor_++; break; }
            error = "expected ',' or ']' in array";
            return false;
        }
        *out = std::move(result);
        return true;
    }

    bool match(const char* literal) {
        std::size_t len = std::strlen(literal);
        if (cursor_ + len > source_.size()) return false;
        if (source_.compare(cursor_, len, literal) != 0) return false;
        cursor_ += len;
        return true;
    }

    const std::string& source_;
    std::size_t cursor_;
};

struct SketchEntity {
    std::string id;
    std::string type;
    double x = 0.0;
    double y = 0.0;
    bool fixed = false;
    std::string start_id;
    std::string end_id;
};

struct SketchConstraint {
    std::string id;
    std::string type;
    std::vector<std::string> entities;
    double value = 0.0;
};

struct SketchRequest {
    std::string schema_version;
    std::string request_id;
    std::vector<SketchEntity> entities;
    std::vector<SketchConstraint> constraints;
};

bool get_string(const JsonParser::Value* obj, const std::string& key, std::string* out) {
    (void)obj;
    (void)key;
    (void)out;
    return false;
}

// Above helpers are placeholders; below we use direct value traversal.

struct JsonObject {
    const std::vector<std::pair<std::string, JsonParser::Value>>* pairs;
};

bool find_pair(const JsonObject& obj, const std::string& key, const JsonParser::Value** out) {
    for (const auto& pair : *obj.pairs) {
        if (pair.first == key) { *out = &pair.second; return true; }
    }
    return false;
}

bool as_string(const JsonParser::Value* value, std::string* out) {
    if (value->kind != JsonParser::ValueKind::String) return false;
    *out = value->string_value;
    return true;
}

bool as_number(const JsonParser::Value* value, double* out) {
    if (value->kind != JsonParser::ValueKind::Number) return false;
    *out = value->number_value;
    return true;
}

bool as_bool(const JsonParser::Value* value, bool* out) {
    if (value->kind != JsonParser::ValueKind::Bool) return false;
    *out = value->bool_value;
    return true;
}

bool as_object(const JsonParser::Value* value, JsonObject* out) {
    if (value->kind != JsonParser::ValueKind::Object) return false;
    out->pairs = &value->object_value;
    return true;
}

bool as_array(const JsonParser::Value* value, std::vector<JsonParser::Value>* out) {
    if (value->kind != JsonParser::ValueKind::Array) return false;
    *out = value->array_value;
    return true;
}

bool parse_request(const std::string& raw, SketchRequest& out, std::string& error) {
    JsonParser parser(raw);
    JsonParser::Value root;
    if (!parser.parse_value(&root, error)) return false;
    if (root.kind != JsonParser::ValueKind::Object) {
        error = "request must be a JSON object";
        return false;
    }
    JsonObject root_obj;
    if (!as_object(&root, &root_obj)) return false;
    const JsonParser::Value* schema_value = nullptr;
    if (!find_pair(root_obj, "schema_version", &schema_value) ||
        !as_string(schema_value, &out.schema_version)) {
        error = "schema_version must be a string";
        return false;
    }
    const JsonParser::Value* id_value = nullptr;
    if (!find_pair(root_obj, "request_id", &id_value) ||
        !as_string(id_value, &out.request_id)) {
        error = "request_id must be a string";
        return false;
    }
    const JsonParser::Value* entities_value = nullptr;
    if (!find_pair(root_obj, "entities", &entities_value)) {
        error = "entities must be present";
        return false;
    }
    std::vector<JsonParser::Value> entities;
    if (!as_array(entities_value, &entities)) {
        error = "entities must be an array";
        return false;
    }
    for (const auto& entity : entities) {
        JsonObject entity_obj;
        if (!as_object(&entity, &entity_obj)) {
            error = "entity must be an object";
            return false;
        }
        SketchEntity sketch_entity;
        const JsonParser::Value* id_node = nullptr;
        if (!find_pair(entity_obj, "id", &id_node) ||
            !as_string(id_node, &sketch_entity.id)) {
            error = "entity.id must be a string";
            return false;
        }
        const JsonParser::Value* type_node = nullptr;
        if (!find_pair(entity_obj, "type", &type_node) ||
            !as_string(type_node, &sketch_entity.type)) {
            error = "entity.type must be a string";
            return false;
        }
        const JsonParser::Value* params_node = nullptr;
        if (!find_pair(entity_obj, "params", &params_node)) {
            error = "entity.params must be present";
            return false;
        }
        JsonObject params_obj;
        if (!as_object(params_node, &params_obj)) {
            error = "entity.params must be an object";
            return false;
        }
        const JsonParser::Value* x_node = nullptr;
        if (find_pair(params_obj, "x", &x_node)) {
            if (!as_number(x_node, &sketch_entity.x)) {
                error = "entity.params.x must be a number";
                return false;
            }
        }
        const JsonParser::Value* y_node = nullptr;
        if (find_pair(params_obj, "y", &y_node)) {
            if (!as_number(y_node, &sketch_entity.y)) {
                error = "entity.params.y must be a number";
                return false;
            }
        }
        const JsonParser::Value* fixed_node = nullptr;
        if (find_pair(params_obj, "fixed", &fixed_node)) {
            if (!as_bool(fixed_node, &sketch_entity.fixed)) {
                error = "entity.params.fixed must be a boolean";
                return false;
            }
        }
        const JsonParser::Value* start_node = nullptr;
        if (find_pair(params_obj, "start", &start_node)) {
            if (!as_string(start_node, &sketch_entity.start_id)) {
                error = "entity.params.start must be a string";
                return false;
            }
        }
        const JsonParser::Value* end_node = nullptr;
        if (find_pair(params_obj, "end", &end_node)) {
            if (!as_string(end_node, &sketch_entity.end_id)) {
                error = "entity.params.end must be a string";
                return false;
            }
        }
        out.entities.push_back(sketch_entity);
    }
    const JsonParser::Value* constraints_value = nullptr;
    if (!find_pair(root_obj, "constraints", &constraints_value)) {
        error = "constraints must be present";
        return false;
    }
    std::vector<JsonParser::Value> constraints;
    if (!as_array(constraints_value, &constraints)) {
        error = "constraints must be an array";
        return false;
    }
    for (const auto& constraint : constraints) {
        JsonObject constraint_obj;
        if (!as_object(&constraint, &constraint_obj)) {
            error = "constraint must be an object";
            return false;
        }
        SketchConstraint sketch_constraint;
        const JsonParser::Value* id_node = nullptr;
        if (!find_pair(constraint_obj, "id", &id_node) ||
            !as_string(id_node, &sketch_constraint.id)) {
            error = "constraint.id must be a string";
            return false;
        }
        const JsonParser::Value* type_node = nullptr;
        if (!find_pair(constraint_obj, "type", &type_node) ||
            !as_string(type_node, &sketch_constraint.type)) {
            error = "constraint.type must be a string";
            return false;
        }
        const JsonParser::Value* entities_node = nullptr;
        if (find_pair(constraint_obj, "entities", &entities_node)) {
            std::vector<JsonParser::Value> refs;
            if (!as_array(entities_node, &refs)) {
                error = "constraint.entities must be an array";
                return false;
            }
            for (const auto& ref : refs) {
                std::string ref_id;
                if (!as_string(&ref, &ref_id)) {
                    error = "constraint.entities element must be a string";
                    return false;
                }
                sketch_constraint.entities.push_back(ref_id);
            }
        }
        const JsonParser::Value* value_node = nullptr;
        if (find_pair(constraint_obj, "value", &value_node)) {
            if (!as_number(value_node, &sketch_constraint.value)) {
                error = "constraint.value must be a number";
                return false;
            }
        }
        out.constraints.push_back(sketch_constraint);
    }
    return true;
}

std::string json_escape(const std::string& input) {
    std::string out;
    out.reserve(input.size() + 2);
    for (char c : input) {
        switch (c) {
            case '"': out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\b': out += "\\b"; break;
            case '\f': out += "\\f"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:
                if (static_cast<unsigned char>(c) < 0x20) {
                    char buffer[8];
                    std::snprintf(buffer, sizeof(buffer), "\\u%04x",
                                  static_cast<unsigned char>(c));
                    out += buffer;
                } else {
                    out.push_back(c);
                }
        }
    }
    return out;
}

void emit_diagnostic(const std::string& code, const std::string& arg) {
    std::ostringstream os;
    os << "{\"code\":\"" << json_escape(code) << "\","
       << "\"arg\":\"" << json_escape(arg) << "\","
       << "\"schema_version\":\"" << kSchemaVersion << "\"}";
    write_stderr_line(os.str());
}

void emit_response(const std::string& request_id, const std::string& status, int dof,
                   const std::vector<std::string>& resolved_ids,
                   const std::vector<std::string>& failed_ids,
                   const std::map<std::string, std::pair<double, double>>& coords) {
    std::ostringstream os;
    os << "{";
    os << "\"schema_version\":\"" << kSchemaVersion << "\",";
    os << "\"request_id\":\"" << json_escape(request_id) << "\",";
    os << "\"status\":\"" << status << "\",";
    os << "\"dof\":" << dof << ",";
    os << "\"resolved_entity_ids\":[";
    for (std::size_t i = 0; i < resolved_ids.size(); ++i) {
        if (i) os << ",";
        os << "\"" << json_escape(resolved_ids[i]) << "\"";
    }
    os << "],";
    os << "\"failed_constraint_ids\":[";
    for (std::size_t i = 0; i < failed_ids.size(); ++i) {
        if (i) os << ",";
        os << "\"" << json_escape(failed_ids[i]) << "\"";
    }
    os << "],";
    os << "\"coordinates\":{";
    bool first = true;
    for (const auto& entry : coords) {
        if (!first) os << ",";
        first = false;
        os << "\"" << json_escape(entry.first) << "\":[" << entry.second.first << ","
           << entry.second.second << "]";
    }
    os << "}}";
    write_stdout_line(os.str());
}

bool validate_id(const std::string& id) {
    if (id.empty() || id.size() > 64) return false;
    for (char c : id) {
        bool ok = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
                  (c >= '0' && c <= '9') || c == '_' || c == '-';
        if (!ok) return false;
    }
    return true;
}

bool validate_request(const SketchRequest& request, std::string& error) {
    if (request.schema_version != kSchemaVersion) {
        error = "schema_version must be \"" + std::string(kSchemaVersion) + "\"";
        return false;
    }
    if (request.request_id.empty()) {
        error = "request_id must not be empty";
        return false;
    }
    std::unordered_set<std::string> seen_ids;
    for (const auto& entity : request.entities) {
        if (!validate_id(entity.id)) {
            error = "entity id \"" + entity.id + "\" is not a valid identifier";
            return false;
        }
        if (!seen_ids.insert(entity.id).second) {
            error = "duplicate entity id \"" + entity.id + "\"";
            return false;
        }
    }
    for (const auto& constraint : request.constraints) {
        if (!validate_id(constraint.id)) {
            error = "constraint id \"" + constraint.id + "\" is not a valid identifier";
            return false;
        }
        if (!seen_ids.insert(constraint.id).second) {
            error = "duplicate id \"" + constraint.id + "\"";
            return false;
        }
        for (const auto& ref : constraint.entities) {
            if (seen_ids.find(ref) == seen_ids.end()) {
                error = "constraint \"" + constraint.id + "\" references unknown entity \"" +
                        ref + "\"";
                return false;
            }
        }
    }
    return true;
}

}  // namespace

int main() {
    std::string raw = read_stdin();
    if (raw.empty()) {
        emit_diagnostic("request_malformed", "empty stdin");
        return 2;
    }
    SketchRequest request;
    std::string error;
    if (!parse_request(raw, request, error)) {
        emit_diagnostic("request_malformed", error);
        return 2;
    }
    if (!validate_request(request, error)) {
        emit_diagnostic("request_malformed", error);
        return 2;
    }

    Slvs_System sys;
    std::memset(&sys, 0, sizeof(sys));

    std::vector<Slvs_Param> slvs_params;
    std::vector<Slvs_Entity> slvs_entities;
    std::vector<Slvs_Constraint> slvs_constraints;
    std::map<std::string, Slvs_Entity> entity_by_id;
    std::map<std::string, Slvs_hConstraint> constraint_handle_by_id;
    std::map<std::string, Slvs_hParam> param_x_by_id;
    std::map<std::string, Slvs_hParam> param_y_by_id;

    Slvs_hParam next_param = 1;
    Slvs_hEntity next_entity = 1;
    Slvs_hConstraint next_constraint = 1;

    auto make_param = [&](double value) {
        Slvs_hParam handle = next_param++;
        Slvs_Param p;
        std::memset(&p, 0, sizeof(p));
        p.h = handle;
        p.group = kGroup;
        p.val = value;
        slvs_params.push_back(p);
        return handle;
    };

    auto add_entity = [&](Slvs_Entity e, const std::string& id) {
        // e.h was assigned by libslvs internally for entities created via
        // helper functions. For entities we built manually, assign a handle
        // here so the global counter is consistent.
        if (e.h == 0) {
            e.h = next_entity++;
        }
        slvs_entities.push_back(e);
        entity_by_id[id] = e;
        return e.h;
    };

    auto add_constraint = [&](Slvs_Constraint c, const std::string& id) {
        // The handle was auto-assigned by libslvs inside Slvs_AddConstraint;
        // preserve it so the failed[] buffer maps back to it correctly.
        slvs_constraints.push_back(c);
        constraint_handle_by_id[id] = c.h;
        return c.h;
    };

    // Implicit XY workplane. libslvs requires a workplane to have an origin
    // point and a normal entity describing orientation. We build all three
    // explicitly so we can include them in slvs_entities.
    // Normal: identity quaternion (w=1, x=0, y=0, z=0).
    Slvs_hParam qw = make_param(1.0);
    Slvs_hParam qx = make_param(0.0);
    Slvs_hParam qy = make_param(0.0);
    Slvs_hParam qz = make_param(0.0);
    Slvs_Entity normal;
    std::memset(&normal, 0, sizeof(normal));
    normal.type = SLVS_E_NORMAL_IN_3D;
    normal.group = kGroup;
    normal.wrkpl = SLVS_FREE_IN_3D;
    normal.param[0] = qw;
    normal.param[1] = qx;
    normal.param[2] = qy;
    normal.param[3] = qz;
    Slvs_hEntity normal_handle = next_entity++;
    normal.h = normal_handle;
    slvs_entities.push_back(normal);

    // Origin: 3D point at (0, 0, 0).
    Slvs_hParam ox = make_param(0.0);
    Slvs_hParam oy = make_param(0.0);
    Slvs_hParam oz = make_param(0.0);
    Slvs_Entity origin;
    std::memset(&origin, 0, sizeof(origin));
    origin.type = SLVS_E_POINT_IN_3D;
    origin.group = kGroup;
    origin.wrkpl = SLVS_FREE_IN_3D;
    origin.param[0] = ox;
    origin.param[1] = oy;
    origin.param[2] = oz;
    Slvs_hEntity origin_handle = next_entity++;
    origin.h = origin_handle;
    slvs_entities.push_back(origin);

    // Workplane.
    Slvs_Entity workplane;
    std::memset(&workplane, 0, sizeof(workplane));
    workplane.type = SLVS_E_WORKPLANE;
    workplane.group = kGroup;
    workplane.wrkpl = SLVS_FREE_IN_3D;
    workplane.point[0] = origin_handle;
    workplane.normal = normal_handle;
    Slvs_hEntity workplane_handle = next_entity++;
    workplane.h = workplane_handle;
    slvs_entities.push_back(workplane);

    Slvs_Entity workplane_entity = workplane;

    // First pass: points (and their params).
    for (const auto& entity : request.entities) {
        if (entity.type == "point_2d") {
            Slvs_hParam px = make_param(entity.x);
            Slvs_hParam py = make_param(entity.y);
            param_x_by_id[entity.id] = px;
            param_y_by_id[entity.id] = py;
            Slvs_Entity point = Slvs_MakePoint2d(SLVS_FREE_IN_3D, kGroup, workplane_handle, px, py);
            Slvs_Entity point_stored = point;
            point_stored.h = next_entity;
            point.h = next_entity;
            next_entity++;
            slvs_entities.push_back(point);
            entity_by_id[entity.id] = point;
            // Anchor the point if requested.
            if (entity.fixed) {
                Slvs_hParam ax = make_param(entity.x);
                Slvs_hParam ay = make_param(entity.y);
                Slvs_hParam az = make_param(0.0);
                Slvs_Entity origin = Slvs_MakePoint3d(SLVS_FREE_IN_3D, kGroup, ax, ay, az);
                Slvs_Entity origin_stored = origin;
                origin_stored.h = next_entity;
                origin.h = next_entity;
                next_entity++;
                slvs_entities.push_back(origin);
                entity_by_id[entity.id + "__anchor__"] = origin;
                Slvs_Constraint fix = Slvs_Coincident(kGroup, point_stored, origin_stored,
                                                      workplane_entity);
                add_constraint(fix, entity.id + "__anchor_constraint__");
            }
        } else if (entity.type != "line_segment_2d") {
            emit_diagnostic("request_malformed",
                            "unsupported entity type \"" + entity.type + "\"");
            return 2;
        }
    }

    // Second pass: line segments, referencing point entities.
    for (const auto& entity : request.entities) {
        if (entity.type != "line_segment_2d") continue;
        auto start_it = entity_by_id.find(entity.start_id);
        auto end_it = entity_by_id.find(entity.end_id);
        if (start_it == entity_by_id.end() || end_it == entity_by_id.end()) {
            emit_diagnostic("internal_error", "line references unknown endpoint");
            return 3;
        }
        Slvs_Entity line;
        std::memset(&line, 0, sizeof(line));
        line.group = kGroup;
        line.type = SLVS_E_LINE_SEGMENT;
        line.wrkpl = workplane_handle;
        line.point[0] = start_it->second.h;
        line.point[1] = end_it->second.h;
        add_entity(line, entity.id);
    }

    // Third pass: constraints.
    auto require_entity = [&](const std::string& id, Slvs_Entity& out) {
        auto it = entity_by_id.find(id);
        if (it == entity_by_id.end()) return false;
        out = it->second;
        return true;
    };
    for (const auto& constraint : request.constraints) {
        Slvs_Constraint c;
        std::memset(&c, 0, sizeof(c));
        c.group = kGroup;
        if (constraint.type == "coincident") {
            if (constraint.entities.size() != 2) {
                emit_diagnostic("request_malformed",
                                "coincident requires two entities, got " +
                                    std::to_string(constraint.entities.size()));
                return 2;
            }
            Slvs_Entity a;
            Slvs_Entity b;
            if (!require_entity(constraint.entities[0], a) ||
                !require_entity(constraint.entities[1], b)) {
                emit_diagnostic("request_malformed", "coincident references unknown entity");
                return 2;
            }
            c = Slvs_Coincident(kGroup, a, b, workplane_entity);
        } else if (constraint.type == "distance") {
            if (constraint.entities.size() != 2) {
                emit_diagnostic("request_malformed",
                                "distance requires two entities, got " +
                                    std::to_string(constraint.entities.size()));
                return 2;
            }
            Slvs_Entity a;
            Slvs_Entity b;
            if (!require_entity(constraint.entities[0], a) ||
                !require_entity(constraint.entities[1], b)) {
                emit_diagnostic("request_malformed", "distance references unknown entity");
                return 2;
            }
            c = Slvs_Distance(kGroup, a, b, constraint.value, workplane_entity);
        } else if (constraint.type == "horizontal") {
            if (constraint.entities.size() != 1) {
                emit_diagnostic("request_malformed",
                                "horizontal requires one entity, got " +
                                    std::to_string(constraint.entities.size()));
                return 2;
            }
            Slvs_Entity a;
            if (!require_entity(constraint.entities[0], a)) {
                emit_diagnostic("request_malformed", "horizontal references unknown entity");
                return 2;
            }
            c = Slvs_Horizontal(kGroup, a, workplane_entity, SLVS_E_NONE);
        } else if (constraint.type == "vertical") {
            if (constraint.entities.size() != 1) {
                emit_diagnostic("request_malformed",
                                "vertical requires one entity, got " +
                                    std::to_string(constraint.entities.size()));
                return 2;
            }
            Slvs_Entity a;
            if (!require_entity(constraint.entities[0], a)) {
                emit_diagnostic("request_malformed", "vertical references unknown entity");
                return 2;
            }
            c = Slvs_Vertical(kGroup, a, workplane_entity, SLVS_E_NONE);
        } else {
            emit_diagnostic("request_malformed",
                            "unsupported constraint type \"" + constraint.type + "\"");
            return 2;
        }
        add_constraint(c, constraint.id);
    }

    sys.param = slvs_params.data();
    sys.params = static_cast<int>(slvs_params.size());
    sys.entity = slvs_entities.data();
    sys.entities = static_cast<int>(slvs_entities.size());
    sys.constraint = slvs_constraints.data();
    sys.constraints = static_cast<int>(slvs_constraints.size());

    std::vector<Slvs_hConstraint> failed_buffers(slvs_constraints.size(), 0);
    sys.calculateFaileds = 1;
    sys.failed = failed_buffers.data();
    sys.faileds = static_cast<int>(failed_buffers.size());

    Slvs_Solve(&sys, kGroup);

    int dof = sys.dof;
    int result = sys.result;
    int faileds = sys.faileds;

    std::string status;
    switch (result) {
        case SLVS_RESULT_OKAY: status = "ok"; break;
        case SLVS_RESULT_INCONSISTENT: status = "inconsistent"; break;
        case SLVS_RESULT_DIDNT_CONVERGE: status = "nonconvergent"; break;
        case SLVS_RESULT_TOO_MANY_UNKNOWNS: status = "rank_deficient"; break;
        case SLVS_RESULT_REDUNDANT_OKAY: status = "redundant_okay"; break;
        default: status = "internal_error"; break;
    }

    std::vector<std::string> failed_constraint_ids;
    if (faileds > 0) {
        // Build a reverse map: libslvs constraint handle -> user id.
        std::map<Slvs_hConstraint, std::string> handle_to_user_id;
        for (const auto& constraint : request.constraints) {
            auto it = constraint_handle_by_id.find(constraint.id);
            if (it != constraint_handle_by_id.end()) {
                handle_to_user_id[it->second] = constraint.id;
            }
        }
        for (int i = 0; i < faileds; ++i) {
            Slvs_hConstraint bad = failed_buffers[i];
            if (bad == 0) continue;
            auto it = handle_to_user_id.find(bad);
            if (it != handle_to_user_id.end()) {
                failed_constraint_ids.push_back(it->second);
            }
        }
    }

    std::vector<std::string> resolved_entity_ids;
    std::map<std::string, std::pair<double, double>> coordinates;
    for (const auto& entity : request.entities) {
        if (entity.type != "point_2d") continue;
        auto px_it = param_x_by_id.find(entity.id);
        auto py_it = param_y_by_id.find(entity.id);
        if (px_it == param_x_by_id.end() || py_it == param_y_by_id.end()) continue;
        bool found_x = false;
        bool found_y = false;
        double xv = 0.0;
        double yv = 0.0;
        for (const auto& p : slvs_params) {
            if (p.h == px_it->second) {
                xv = p.val;
                found_x = true;
            }
            if (p.h == py_it->second) {
                yv = p.val;
                found_y = true;
            }
        }
        if (found_x && found_y) {
            double rounded_x = std::round(xv * 1e9) / 1e9;
            double rounded_y = std::round(yv * 1e9) / 1e9;
            coordinates[entity.id] = std::make_pair(rounded_x, rounded_y);
            resolved_entity_ids.push_back(entity.id);
        }
    }

    emit_response(request.request_id, status, dof, resolved_entity_ids,
                  failed_constraint_ids, coordinates);
    return 0;
}