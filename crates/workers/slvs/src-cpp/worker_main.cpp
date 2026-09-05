// SPDX-License-Identifier: GPL-3.0-only
//
// ThreeTerm's disposable SolveSpace libslvs worker. The solver owns no
// ThreeTerm state: stable IDs are mapped to private numeric handles for one
// request and are mapped back only in the completed response.

#include <slvs.h>

#include <cmath>
#include <cctype>
#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <map>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

namespace {

constexpr const char* kProtocolSchema = "threeterm.protocol/1";
constexpr const char* kWorkerSchema = "threeterm.workers.slvs/1";
constexpr const char* kOperation = "sketch_solve";

struct Json {
    enum class Kind { Null, Bool, Number, String, Array, Object } kind = Kind::Null;
    double number = 0.0;
    bool boolean = false;
    std::string string;
    std::vector<Json> array;
    std::map<std::string, Json> object;
};

class Parser {
public:
    explicit Parser(const std::string& input) : input_(input) {}

    bool parse(Json& value, std::string& error) {
        skip_space();
        if (!value_at(value, error) || !consume_end(error)) return false;
        return true;
    }

private:
    bool value_at(Json& value, std::string& error) {
        skip_space();
        if (cursor_ >= input_.size()) return fail(error, "unexpected end of JSON");
        switch (input_[cursor_]) {
            case '{': return object(value, error);
            case '[': return array(value, error);
            case '"':
                value.kind = Json::Kind::String;
                return string(value.string, error);
            case 't': return literal(value, "true", true, error);
            case 'f': return literal(value, "false", false, error);
            case 'n':
                if (input_.compare(cursor_, 4, "null") != 0) return fail(error, "invalid null");
                cursor_ += 4;
                value.kind = Json::Kind::Null;
                return true;
            default: return number(value, error);
        }
    }

    bool object(Json& value, std::string& error) {
        ++cursor_;
        value.kind = Json::Kind::Object;
        skip_space();
        if (take('}')) return true;
        while (cursor_ < input_.size()) {
            std::string key;
            if (!string(key, error)) return false;
            skip_space();
            if (!take(':')) return fail(error, "object member requires colon");
            Json member;
            if (!value_at(member, error)) return false;
            value.object[key] = std::move(member);
            skip_space();
            if (take('}')) return true;
            if (!take(',')) return fail(error, "object member requires comma");
            skip_space();
        }
        return fail(error, "unterminated object");
    }

    bool array(Json& value, std::string& error) {
        ++cursor_;
        value.kind = Json::Kind::Array;
        skip_space();
        if (take(']')) return true;
        while (cursor_ < input_.size()) {
            Json item;
            if (!value_at(item, error)) return false;
            value.array.push_back(std::move(item));
            skip_space();
            if (take(']')) return true;
            if (!take(',')) return fail(error, "array item requires comma");
            skip_space();
        }
        return fail(error, "unterminated array");
    }

    bool string(std::string& value, std::string& error) {
        if (!take('"')) return fail(error, "expected string");
        value.clear();
        while (cursor_ < input_.size()) {
            const char character = input_[cursor_++];
            if (character == '"') return true;
            if (character != '\\') {
                value.push_back(character);
                continue;
            }
            if (cursor_ >= input_.size()) return fail(error, "unterminated escape");
            const char escaped = input_[cursor_++];
            switch (escaped) {
                case '"': value.push_back('"'); break;
                case '\\': value.push_back('\\'); break;
                case '/': value.push_back('/'); break;
                case 'b': value.push_back('\b'); break;
                case 'f': value.push_back('\f'); break;
                case 'n': value.push_back('\n'); break;
                case 'r': value.push_back('\r'); break;
                case 't': value.push_back('\t'); break;
                default: return fail(error, "unsupported string escape");
            }
        }
        return fail(error, "unterminated string");
    }

    bool number(Json& value, std::string& error) {
        const std::size_t start = cursor_;
        while (cursor_ < input_.size() && std::string("-+0123456789.eE").find(input_[cursor_]) != std::string::npos) ++cursor_;
        if (start == cursor_) return fail(error, "expected JSON value");
        try {
            std::size_t consumed = 0;
            value.number = std::stod(input_.substr(start), &consumed);
            if (consumed != cursor_ - start || !std::isfinite(value.number)) return fail(error, "number must be finite");
        } catch (...) {
            return fail(error, "invalid number");
        }
        value.kind = Json::Kind::Number;
        return true;
    }

    bool literal(Json& value, const char* text, bool boolean, std::string& error) {
        const std::size_t length = std::string(text).size();
        if (input_.compare(cursor_, length, text) != 0) return fail(error, "invalid literal");
        cursor_ += length;
        value.kind = Json::Kind::Bool;
        value.boolean = boolean;
        return true;
    }

    bool consume_end(std::string& error) {
        skip_space();
        return cursor_ == input_.size() || fail(error, "trailing JSON data");
    }

    void skip_space() {
        while (cursor_ < input_.size() && std::isspace(static_cast<unsigned char>(input_[cursor_]))) ++cursor_;
    }

    bool take(char expected) {
        if (cursor_ < input_.size() && input_[cursor_] == expected) {
            ++cursor_;
            return true;
        }
        return false;
    }

    bool fail(std::string& error, const char* detail) {
        error = detail;
        return false;
    }

    const std::string& input_;
    std::size_t cursor_ = 0;
};

const Json* field(const Json& value, const char* name) {
    if (value.kind != Json::Kind::Object) return nullptr;
    auto found = value.object.find(name);
    return found == value.object.end() ? nullptr : &found->second;
}

std::string string_field(const Json& value, const char* name) {
    const Json* found = field(value, name);
    return found != nullptr && found->kind == Json::Kind::String ? found->string : std::string{};
}

std::string escape(const std::string& value);

std::string vec3_field(const Json& value, const char* name) {
    const Json* found = field(value, name);
    if (found == nullptr || found->kind != Json::Kind::Array || found->array.size() != 3) return "[]";
    std::ostringstream output;
    output << '[' << found->array[0].number << ',' << found->array[1].number << ','
           << found->array[2].number << ']';
    return output.str();
}

std::string attachment_field(const Json& args, const char* name) {
    const Json* found = field(args, name);
    if (found == nullptr || found->kind != Json::Kind::Object) return "null";
    std::ostringstream output;
    output << "{\"semantic_id\":\"" << escape(string_field(*found, "semantic_id"))
           << "\",\"role\":\"" << escape(string_field(*found, "role")) << "\",\"provenance\":{";
    const Json* provenance = field(*found, "provenance");
    output << "\"source_feature_id\":\""
           << escape(provenance == nullptr ? "" : string_field(*provenance, "source_feature_id"))
           << "\",\"source_revision_id\":\""
           << escape(provenance == nullptr ? "" : string_field(*provenance, "source_revision_id"))
           << "\",\"source_face_id\":\""
           << escape(provenance == nullptr ? "" : string_field(*provenance, "source_face_id"))
           << "},\"evidence\":{";
    const Json* evidence = field(*found, "evidence");
    output << "\"topology_kind\":\""
           << escape(evidence == nullptr ? "" : string_field(*evidence, "topology_kind"))
           << "\",\"origin\":" << (evidence == nullptr ? "[]" : vec3_field(*evidence, "origin"))
           << ",\"normal\":" << (evidence == nullptr ? "[]" : vec3_field(*evidence, "normal"))
           << ",\"x_axis\":" << (evidence == nullptr ? "[]" : vec3_field(*evidence, "x_axis"))
           << ",\"y_axis\":" << (evidence == nullptr ? "[]" : vec3_field(*evidence, "y_axis"))
           << ",\"adjacent_feature_ids\":[]}}";
    return output.str();
}

double number_field(const Json& value, const char* name, double fallback = 0.0) {
    const Json* found = field(value, name);
    return found != nullptr && found->kind == Json::Kind::Number ? found->number : fallback;
}

std::string escape(const std::string& value) {
    std::ostringstream output;
    for (const char character : value) {
        switch (character) {
            case '"': output << "\\\""; break;
            case '\\': output << "\\\\"; break;
            case '\n': output << "\\n"; break;
            case '\r': output << "\\r"; break;
            case '\t': output << "\\t"; break;
            default: output << character; break;
        }
    }
    return output.str();
}

void ready() {
    std::cout << "{\"kind\":\"worker_ready\",\"schema_version\":\"" << kProtocolSchema
              << "\",\"worker_id\":\"slvs\"}\n" << std::flush;
}

void failed(const std::string& request_id, const std::string& code, const std::string& detail) {
    std::cout << "{\"kind\":\"failed\",\"schema_version\":\"" << kProtocolSchema
              << "\",\"request_id\":\"" << escape(request_id) << "\",\"code\":\""
              << escape(code) << "\",\"detail\":\"" << escape(detail) << "\"}\n" << std::flush;
}

struct PointState {
    std::string id;
    Slvs_hEntity handle = 0;
    Slvs_hParam x = 0;
    Slvs_hParam y = 0;
};

struct EntityState {
    std::string id;
    Slvs_hEntity handle = 0;
};

int constraint_type(const std::string& kind) {
    if (kind == "coincident") return SLVS_C_POINTS_COINCIDENT;
    if (kind == "distance") return SLVS_C_PT_PT_DISTANCE;
    if (kind == "horizontal") return SLVS_C_HORIZONTAL;
    if (kind == "vertical") return SLVS_C_VERTICAL;
    if (kind == "equal_length") return SLVS_C_EQUAL_LENGTH_LINES;
    if (kind == "parallel") return SLVS_C_PARALLEL;
    if (kind == "perpendicular") return SLVS_C_PERPENDICULAR;
    if (kind == "fixed") return SLVS_C_WHERE_DRAGGED;
    return 0;
}

std::string status_name(int result, int dof) {
    if (result == SLVS_RESULT_OKAY) return dof == 0 ? "solved" : "underconstrained";
    if (result == SLVS_RESULT_REDUNDANT_OKAY) return "redundant";
    if (result == SLVS_RESULT_INCONSISTENT) return "inconsistent";
    if (result == SLVS_RESULT_DIDNT_CONVERGE) return "nonconvergent";
    return "invalid_request";
}

bool solve(const Json& args, const std::string& request_id, std::string& result, std::string& error) {
    if (string_field(args, "schema_version") != kWorkerSchema ||
        string_field(args, "request_id") != request_id ||
        string_field(args, "operation") != kOperation) {
        error = "worker request identity or schema mismatch";
        return false;
    }
    const Json* entities_json = field(args, "entities");
    const Json* constraints_json = field(args, "constraints");
    if (entities_json == nullptr || entities_json->kind != Json::Kind::Array ||
        constraints_json == nullptr || constraints_json->kind != Json::Kind::Array ||
        entities_json->array.empty()) {
        error = "entities and constraints must be arrays and entities must not be empty";
        return false;
    }

    constexpr Slvs_hGroup base_group = 1;
    constexpr Slvs_hGroup group = 2;
    constexpr Slvs_hEntity origin = 1;
    constexpr Slvs_hEntity normal = 2;
    constexpr Slvs_hEntity workplane = 3;
    std::vector<Slvs_Param> params;
    std::vector<Slvs_Entity> entities;
    std::vector<PointState> points;
    std::vector<EntityState> other_entities;
    std::map<std::string, Slvs_hEntity> handles;
    std::map<std::string, std::size_t> point_indexes;
    params.push_back(Slvs_MakeParam(1, base_group, 0.0));
    params.push_back(Slvs_MakeParam(2, base_group, 0.0));
    params.push_back(Slvs_MakeParam(3, base_group, 0.0));
    params.push_back(Slvs_MakeParam(4, base_group, 1.0));
    params.push_back(Slvs_MakeParam(5, base_group, 0.0));
    params.push_back(Slvs_MakeParam(6, base_group, 0.0));
    params.push_back(Slvs_MakeParam(7, base_group, 0.0));
    entities.push_back(Slvs_MakePoint3d(origin, base_group, 1, 2, 3));
    entities.push_back(Slvs_MakeNormal3d(normal, base_group, 4, 5, 6, 7));
    entities.push_back(Slvs_MakeWorkplane(workplane, base_group, origin, normal));

    Slvs_hEntity next_entity = 10;
    Slvs_hParam next_param = 10;
    for (const Json& json_entity : entities_json->array) {
        const std::string kind = string_field(json_entity, "kind");
        const std::string id = string_field(json_entity, "id");
        if (id.empty() || handles.count(id) != 0) { error = "entity IDs must be unique"; return false; }
        if (kind == "point") {
            const Slvs_hParam x = next_param++;
            const Slvs_hParam y = next_param++;
            const Slvs_hEntity handle = next_entity++;
            const double x_value = number_field(json_entity, "x", NAN);
            const double y_value = number_field(json_entity, "y", NAN);
            if (!std::isfinite(x_value) || !std::isfinite(y_value)) { error = "point coordinates must be finite"; return false; }
            params.push_back(Slvs_MakeParam(x, group, x_value));
            params.push_back(Slvs_MakeParam(y, group, y_value));
            entities.push_back(Slvs_MakePoint2d(handle, group, workplane, x, y));
            point_indexes[id] = points.size();
            points.push_back({id, handle, x, y});
            handles[id] = handle;
        } else if (kind == "line_segment") {
            const std::string start = string_field(json_entity, "start");
            const std::string end = string_field(json_entity, "end");
            if (handles.count(start) == 0 || handles.count(end) == 0) { error = "line references an unknown point"; return false; }
            const Slvs_hEntity handle = next_entity++;
            entities.push_back(Slvs_MakeLineSegment(handle, group, workplane, handles[start], handles[end]));
            other_entities.push_back({id, handle});
            handles[id] = handle;
        } else if (kind == "circle") {
            const std::string center = string_field(json_entity, "center");
            const double radius = number_field(json_entity, "radius", NAN);
            if (handles.count(center) == 0 || !std::isfinite(radius) || radius <= 0.0) {
                error = "circle requires a known center and positive finite radius";
                return false;
            }
            const Slvs_hEntity circle_normal = next_entity++;
            const Slvs_hEntity radius_entity = next_entity++;
            const Slvs_hParam radius_param = next_param++;
            const Slvs_hEntity handle = next_entity++;
            params.push_back(Slvs_MakeParam(radius_param, group, radius));
            entities.push_back(Slvs_MakeNormal2d(circle_normal, group, workplane));
            entities.push_back(Slvs_MakeDistance(radius_entity, group, workplane, radius_param));
            entities.push_back(Slvs_MakeCircle(handle, group, workplane, handles[center], circle_normal, radius_entity));
            other_entities.push_back({id, handle});
            handles[id] = handle;
        } else if (kind == "arc") {
            const std::string center = string_field(json_entity, "center");
            const std::string start = string_field(json_entity, "start");
            const std::string end = string_field(json_entity, "end");
            if (handles.count(center) == 0 || handles.count(start) == 0 || handles.count(end) == 0) {
                error = "arc references an unknown point";
                return false;
            }
            const Slvs_hEntity arc_normal = next_entity++;
            const Slvs_hEntity handle = next_entity++;
            entities.push_back(Slvs_MakeNormal2d(arc_normal, group, workplane));
            entities.push_back(Slvs_MakeArcOfCircle(
                handle, group, workplane, arc_normal, handles[center], handles[start], handles[end]));
            other_entities.push_back({id, handle});
            handles[id] = handle;
        } else {
            error = "unsupported sketch entity kind: " + kind;
            return false;
        }
    }

    std::vector<Slvs_Constraint> constraints;
    std::vector<std::string> constraint_ids;
    std::map<Slvs_hConstraint, std::string> constraint_names;
    Slvs_hConstraint next_constraint = 100;
    for (const Json& json_constraint : constraints_json->array) {
        const std::string id = string_field(json_constraint, "id");
        const std::string kind = string_field(json_constraint, "kind");
        const Json* refs = field(json_constraint, "entities");
        if (id.empty() || refs == nullptr || refs->kind != Json::Kind::Array) { error = "constraint requires id and entities"; return false; }
        const int type = constraint_type(kind);
        if (type == 0) { error = "unsupported constraint kind: " + kind; return false; }
        std::vector<Slvs_hEntity> referenced;
        for (const Json& ref : refs->array) {
            if (ref.kind != Json::Kind::String || handles.count(ref.string) == 0) { error = "constraint references an unknown entity"; return false; }
            referenced.push_back(handles[ref.string]);
        }
        Slvs_Constraint constraint = Slvs_MakeConstraint(
            next_constraint++, group, type, workplane, number_field(json_constraint, "value"),
            0, 0, 0, 0);
        if (type == SLVS_C_WHERE_DRAGGED && referenced.size() != 1) { error = "fixed requires one point"; return false; }
        if ((type == SLVS_C_HORIZONTAL || type == SLVS_C_VERTICAL) && referenced.size() != 1) { error = kind + " requires one line"; return false; }
        if ((type == SLVS_C_EQUAL_LENGTH_LINES || type == SLVS_C_PARALLEL || type == SLVS_C_PERPENDICULAR) && referenced.size() != 2) { error = kind + " requires two lines"; return false; }
        if ((type == SLVS_C_POINTS_COINCIDENT || type == SLVS_C_PT_PT_DISTANCE) && referenced.size() != 2) { error = kind + " requires two points"; return false; }
        if (type == SLVS_C_WHERE_DRAGGED) {
            constraint.ptA = referenced[0];
        } else if (type == SLVS_C_HORIZONTAL || type == SLVS_C_VERTICAL) {
            constraint.entityA = referenced[0];
        } else if (type == SLVS_C_POINTS_COINCIDENT || type == SLVS_C_PT_PT_DISTANCE) {
            constraint.ptA = referenced[0];
            constraint.ptB = referenced[1];
        } else {
            constraint.entityA = referenced[0];
            constraint.entityB = referenced[1];
        }
        constraints.push_back(constraint);
        constraint_ids.push_back(id);
        constraint_names[constraint.h] = id;
    }

    std::vector<Slvs_hConstraint> failed_handles(constraints.size());
    Slvs_System system{};
    system.param = params.data();
    system.params = static_cast<int>(params.size());
    system.entity = entities.data();
    system.entities = static_cast<int>(entities.size());
    system.constraint = constraints.data();
    system.constraints = static_cast<int>(constraints.size());
    system.calculateFaileds = 1;
    system.failed = failed_handles.data();
    system.faileds = static_cast<int>(failed_handles.size());
    Slvs_Solve(&system, group);

    const std::string status = status_name(system.result, system.dof);
    std::vector<std::string> related;
    for (int index = 0; index < system.faileds && index < static_cast<int>(failed_handles.size()); ++index) {
        auto found = constraint_names.find(failed_handles[static_cast<std::size_t>(index)]);
        if (found != constraint_names.end()) related.push_back(found->second);
    }
    std::ostringstream output;
    output << "{\"schema_version\":\"" << kWorkerSchema << "\",\"request_id\":\"" << escape(request_id)
           << "\",\"operation\":\"" << kOperation << "\",\"feature_id\":\"" << escape(string_field(args, "feature_id"))
           << "\",\"source_revision\":\"" << escape(string_field(args, "source_revision"))
           << "\",\"status\":\"" << status << "\",\"dof\":" << system.dof << ",\"entity_ids\":[";
    for (std::size_t index = 0; index < entities_json->array.size(); ++index) {
        if (index != 0) output << ',';
        output << '"' << escape(string_field(entities_json->array[index], "id")) << '"';
    }
    output << "],\"related_constraint_ids\":[";
    for (std::size_t index = 0; index < related.size(); ++index) {
        if (index != 0) output << ',';
        output << '"' << escape(related[index]) << '"';
    }
    output << "],\"diagnostics\":[";
    if (status != "solved") {
        output << "{\"code\":\"solver_" << status << "\",\"detail\":\"libslvs returned " << status << "\",\"constraint_ids\":[";
        for (std::size_t index = 0; index < related.size(); ++index) {
            if (index != 0) output << ',';
            output << '"' << escape(related[index]) << '"';
        }
        output << "]}";
    }
    output << "]";
    if (field(args, "support") != nullptr && field(args, "placement") != nullptr) {
        output << ",\"support\":" << attachment_field(args, "support")
               << ",\"placement\":{";
        const Json* placement = field(args, "placement");
        output << "\"origin\":" << vec3_field(*placement, "origin")
               << ",\"x_axis\":" << vec3_field(*placement, "x_axis")
               << ",\"y_axis\":" << vec3_field(*placement, "y_axis")
               << ",\"normal\":" << vec3_field(*placement, "normal") << '}';
    }
    if (status == "solved") {
        output << ",\"solved_coordinates\":[";
        for (std::size_t index = 0; index < points.size(); ++index) {
            if (index != 0) output << ',';
            const auto& point = points[index];
            double x = 0.0;
            double y = 0.0;
            for (const auto& parameter : params) {
                if (parameter.h == point.x) x = parameter.val;
                if (parameter.h == point.y) y = parameter.val;
            }
            output << "{\"entity_id\":\"" << escape(point.id) << "\",\"x\":" << x << ",\"y\":" << y << '}';
        }
        output << ']';
    }
    output << '}';
    result = output.str();
    return true;
}

} // namespace

int main() {
    ready();
    std::string line;
    if (!std::getline(std::cin, line) || line.empty()) {
        failed("", "request_malformed", "request line must be newline terminated");
        return 2;
    }
    Json envelope;
    std::string error;
    Parser parser(line);
    if (!parser.parse(envelope, error) || string_field(envelope, "kind") != "request" ||
        string_field(envelope, "schema_version") != kProtocolSchema) {
        failed(string_field(envelope, "request_id"), "request_malformed", error.empty() ? "invalid request envelope" : error);
        return 2;
    }
    const std::string request_id = string_field(envelope, "request_id");
    if (string_field(envelope, "command_id") != kOperation) {
        failed(request_id, "request_malformed", "unknown command_id");
        return 2;
    }
    const Json* args = field(envelope, "args");
    if (args == nullptr) {
        failed(request_id, "request_malformed", "request envelope is missing args");
        return 2;
    }
    std::cout << "{\"kind\":\"progress\",\"schema_version\":\"" << kProtocolSchema
              << "\",\"request_id\":\"" << escape(request_id) << "\",\"stage\":\"solving\",\"percent\":50}\n" << std::flush;
    std::string result;
    if (!solve(*args, request_id, result, error)) {
        failed(request_id, "request_malformed", error);
        return 2;
    }
    std::cout << "{\"kind\":\"completed\",\"schema_version\":\"" << kProtocolSchema
              << "\",\"request_id\":\"" << escape(request_id) << "\",\"result\":" << result << "}\n" << std::flush;
    return 0;
}
