// SPDX-License-Identifier: LGPL-2.1-or-later
//
// threeterm-occt-worker: disposable worker binary for the ThreeTerm OCCT
// geometry kernel. Reads a JSON envelope from stdin, runs the requested
// operation (extrude, boolean_fuse, fillet, chamfer, hole, revolve,
// mirror, linear_pattern, circular_pattern, shell, draft, or loft),
// validates the BREP with BRepCheck_Analyzer, and writes the BREP to the
// host-staged output path.
//
// Exit codes:
//   0  success — BREP written, JSON response on stdout.
//   2  request malformed — JSON envelope missing fields, profile has
//      fewer than 3 vertices, BREP file could not be read, etc.
//   3  brep_invalid — the operation produced a BREP that fails
//      BRepCheck_Analyzer (the BREP is still written so the host can
//      surface the diagnostic and choose its own recovery).
//   other non-zero — internal OCCT failure; the diagnostic on stderr
//      is the human-readable cause.
//
// This file is part of ThreeTerm; see ../NOTICE for upstream provenance
// and the LGPL-2.1 redistribution obligations.

#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBndLib.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakePolygon.hxx>
#include <BRepBuilderAPI_MakeVertex.hxx>
#include <BRepBuilderAPI_Transform.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepGProp.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRepPrimAPI_MakeRevol.hxx>
#include <BRep_Builder.hxx>
#include <BRepTools.hxx>
#include <Bnd_Box.hxx>
#include <GProp_GProps.hxx>
#include <Message_ProgressRange.hxx>
#include <Standard_IStream.hxx>
#include <Standard_Failure.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_CompSolid.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Solid.hxx>
#include <gp_Ax1.hxx>
#include <gp_Ax2.hxx>
#include <gp_Dir.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>
#include <gp_Vec.hxx>

#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>

#include <BRepBuilderAPI_MakeSolid.hxx>
#include <BRepOffsetAPI_DraftAngle.hxx>
#include <BRepOffsetAPI_MakeThickSolid.hxx>
#include <BRepOffsetAPI_ThruSections.hxx>
#include <BRep_Tool.hxx>
#include <Geom_Plane.hxx>
#include <ShapeUpgrade_UnifySameDomain.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS_Face.hxx>

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <map>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

namespace {

constexpr const char* kSchemaVersion = "threeterm.workers.occt/1";

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

class JsonParser {
public:
    enum class ValueKind { Object, Array, String, Number, Bool, Null };

    struct Value {
        ValueKind kind = ValueKind::Null;
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
        if (c == '-' || (c >= '0' && c <= '9')) return parse_number(out, error);
        error = std::string{"unexpected character in JSON: "} + c;
        return false;
    }

private:
    const std::string& source_;
    std::size_t cursor_;

    bool at_end() const { return cursor_ >= source_.size(); }
    char peek() const { return source_[cursor_]; }
    void skip_ws() {
        while (!at_end() && (source_[cursor_] == ' ' || source_[cursor_] == '\t' ||
                              source_[cursor_] == '\n' || source_[cursor_] == '\r')) {
            cursor_++;
        }
    }
    bool expect(char c, std::string& error) {
        skip_ws();
        if (at_end() || source_[cursor_] != c) {
            error = std::string{"expected '"} + c + "'";
            return false;
        }
        cursor_++;
        return true;
    }

    bool parse_object(Value* out, std::string& error) {
        if (!expect('{', error)) return false;
        out->kind = ValueKind::Object;
        skip_ws();
        if (!at_end() && source_[cursor_] == '}') { cursor_++; return true; }
        while (true) {
            skip_ws();
            std::string key;
            if (!parse_string_raw(&key, error)) return false;
            if (!expect(':', error)) return false;
            Value value;
            if (!parse_value(&value, error)) return false;
            out->object_value.emplace_back(std::move(key), std::move(value));
            skip_ws();
            if (at_end()) { error = "unterminated object"; return false; }
            if (source_[cursor_] == ',') { cursor_++; continue; }
            if (source_[cursor_] == '}') { cursor_++; return true; }
            error = "expected ',' or '}'";
            return false;
        }
    }

    bool parse_array(Value* out, std::string& error) {
        if (!expect('[', error)) return false;
        out->kind = ValueKind::Array;
        skip_ws();
        if (!at_end() && source_[cursor_] == ']') { cursor_++; return true; }
        while (true) {
            Value value;
            if (!parse_value(&value, error)) return false;
            out->array_value.push_back(std::move(value));
            skip_ws();
            if (at_end()) { error = "unterminated array"; return false; }
            if (source_[cursor_] == ',') { cursor_++; continue; }
            if (source_[cursor_] == ']') { cursor_++; return true; }
            error = "expected ',' or ']'";
            return false;
        }
    }

    bool parse_string(Value* out, std::string& error) {
        out->kind = ValueKind::String;
        return parse_string_raw(&out->string_value, error);
    }

    bool parse_string_raw(std::string* out, std::string& error) {
        if (!expect('"', error)) return false;
        while (true) {
            if (at_end()) { error = "unterminated string"; return false; }
            char c = source_[cursor_++];
            if (c == '"') return true;
            if (c == '\\') {
                if (at_end()) { error = "dangling escape"; return false; }
                char esc = source_[cursor_++];
                switch (esc) {
                    case '"': out->push_back('"'); break;
                    case '\\': out->push_back('\\'); break;
                    case '/': out->push_back('/'); break;
                    case 'b': out->push_back('\b'); break;
                    case 'f': out->push_back('\f'); break;
                    case 'n': out->push_back('\n'); break;
                    case 'r': out->push_back('\r'); break;
                    case 't': out->push_back('\t'); break;
                    case 'u': {
                        if (cursor_ + 4 > source_.size()) {
                            error = "short \\u escape";
                            return false;
                        }
                        unsigned int code = 0;
                        for (int i = 0; i < 4; ++i) {
                            char h = source_[cursor_ + i];
                            unsigned int digit = 0;
                            if (h >= '0' && h <= '9') digit = h - '0';
                            else if (h >= 'a' && h <= 'f') digit = 10 + (h - 'a');
                            else if (h >= 'A' && h <= 'F') digit = 10 + (h - 'A');
                            else { error = "bad hex in \\u escape"; return false; }
                            code = (code << 4) | digit;
                        }
                        cursor_ += 4;
                        if (code < 0x80) {
                            out->push_back(static_cast<char>(code));
                        } else if (code < 0x800) {
                            out->push_back(static_cast<char>(0xC0 | (code >> 6)));
                            out->push_back(static_cast<char>(0x80 | (code & 0x3F)));
                        } else {
                            out->push_back(static_cast<char>(0xE0 | (code >> 12)));
                            out->push_back(static_cast<char>(0x80 | ((code >> 6) & 0x3F)));
                            out->push_back(static_cast<char>(0x80 | (code & 0x3F)));
                        }
                        break;
                    }
                    default:
                        error = std::string{"unsupported escape: "} + esc;
                        return false;
                }
            } else {
                out->push_back(c);
            }
        }
    }

    bool parse_number(Value* out, std::string& error) {
        std::size_t start = cursor_;
        if (!at_end() && source_[cursor_] == '-') cursor_++;
        while (!at_end() &&
               ((source_[cursor_] >= '0' && source_[cursor_] <= '9') ||
                source_[cursor_] == '.' || source_[cursor_] == 'e' || source_[cursor_] == 'E' ||
                source_[cursor_] == '+' || source_[cursor_] == '-')) {
            cursor_++;
        }
        std::string text = source_.substr(start, cursor_ - start);
        if (text.empty()) { error = "empty number"; return false; }
        try {
            out->number_value = std::stod(text);
        } catch (...) {
            error = "could not parse number: " + text;
            return false;
        }
        out->kind = ValueKind::Number;
        return true;
    }

    bool parse_bool(Value* out, std::string& error) {
        if (cursor_ + 4 <= source_.size() && source_.compare(cursor_, 4, "true") == 0) {
            cursor_ += 4;
            out->kind = ValueKind::Bool;
            out->bool_value = true;
            return true;
        }
        if (cursor_ + 5 <= source_.size() && source_.compare(cursor_, 5, "false") == 0) {
            cursor_ += 5;
            out->kind = ValueKind::Bool;
            out->bool_value = false;
            return true;
        }
        error = "expected boolean";
        return false;
    }

    bool parse_null(Value* out, std::string& error) {
        if (cursor_ + 4 <= source_.size() && source_.compare(cursor_, 4, "null") == 0) {
            cursor_ += 4;
            out->kind = ValueKind::Null;
            return true;
        }
        error = "expected null";
        return false;
    }
};

const JsonParser::Value* find_field(const JsonParser::Value& object, const std::string& key) {
    if (object.kind != JsonParser::ValueKind::Object) return nullptr;
    for (const auto& pair : object.object_value) {
        if (pair.first == key) return &pair.second;
    }
    return nullptr;
}

std::string get_string(const JsonParser::Value& object, const std::string& key) {
    const auto* value = find_field(object, key);
    if (value == nullptr || value->kind != JsonParser::ValueKind::String) return std::string{};
    return value->string_value;
}

double get_number(const JsonParser::Value& object, const std::string& key) {
    const auto* value = find_field(object, key);
    if (value == nullptr || value->kind != JsonParser::ValueKind::Number) return 0.0;
    return value->number_value;
}

std::vector<std::array<double, 2>> get_profile(const JsonParser::Value& object,
                                                const std::string& key) {
    std::vector<std::array<double, 2>> result;
    const auto* value = find_field(object, key);
    if (value == nullptr || value->kind != JsonParser::ValueKind::Array) return result;
    result.reserve(value->array_value.size());
    for (const auto& pair : value->array_value) {
        if (pair.kind != JsonParser::ValueKind::Array) continue;
        if (pair.array_value.size() != 2) continue;
        if (pair.array_value[0].kind != JsonParser::ValueKind::Number) continue;
        if (pair.array_value[1].kind != JsonParser::ValueKind::Number) continue;
        result.push_back(
            {pair.array_value[0].number_value, pair.array_value[1].number_value});
    }
    return result;
}

std::array<double, 3> get_vec3(const JsonParser::Value& object, const std::string& key) {
    std::array<double, 3> result{0.0, 0.0, 0.0};
    const auto* value = find_field(object, key);
    if (value == nullptr || value->kind != JsonParser::ValueKind::Array) return result;
    if (value->array_value.size() != 3) return result;
    for (std::size_t index = 0; index < 3; ++index) {
        if (value->array_value[index].kind != JsonParser::ValueKind::Number) return result;
        result[index] = value->array_value[index].number_value;
    }
    return result;
}

bool get_bool(const JsonParser::Value& object, const std::string& key, bool default_value,
              std::string& error) {
    const auto* value = find_field(object, key);
    if (value == nullptr) return default_value;
    if (value->kind == JsonParser::ValueKind::Bool) return value->bool_value;
    error = key + " must be a boolean";
    return default_value;
}

bool get_profiles(const JsonParser::Value& object, const std::string& key,
                  std::vector<std::vector<std::array<double, 3>>>& result, std::string& error) {
    result.clear();
    const auto* value = find_field(object, key);
    if (value == nullptr || value->kind != JsonParser::ValueKind::Array) {
        error = "loft profiles must be an array";
        return false;
    }
    result.reserve(value->array_value.size());
    for (std::size_t profile_index = 0; profile_index < value->array_value.size(); ++profile_index) {
        const auto& profile_value = value->array_value[profile_index];
        if (profile_value.kind != JsonParser::ValueKind::Array) {
            error = "loft profile " + std::to_string(profile_index) + " must be an array";
            return false;
        }
        std::vector<std::array<double, 3>> profile;
        profile.reserve(profile_value.array_value.size());
        for (std::size_t vertex_index = 0; vertex_index < profile_value.array_value.size(); ++vertex_index) {
            const auto& vertex = profile_value.array_value[vertex_index];
            if (vertex.kind != JsonParser::ValueKind::Array || vertex.array_value.size() != 3) {
                error = "loft profile " + std::to_string(profile_index) + " vertex " +
                        std::to_string(vertex_index) + " must be an array of exactly 3 numbers";
                return false;
            }
            for (const auto& coordinate : vertex.array_value) {
                if (coordinate.kind != JsonParser::ValueKind::Number) {
                    error = "loft profile " + std::to_string(profile_index) + " vertex " +
                            std::to_string(vertex_index) + " must contain only numbers";
                    return false;
                }
            }
            profile.push_back({vertex.array_value[0].number_value,
                               vertex.array_value[1].number_value,
                               vertex.array_value[2].number_value});
        }
        result.push_back(std::move(profile));
    }
    return true;
}

std::string sha256_hex(const std::string& bytes) {
    // Inline 32-bit right-rotation. `std::rotr` is C++20; this slice
    // builds with `-std=c++17`, so we provide a small helper.
    auto rotr = [](std::uint32_t value, std::uint32_t bits) -> std::uint32_t {
        return (value >> bits) | (value << (32 - bits));
    };
    static const std::uint32_t k[64] = {
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1,
        0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
        0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2};
    std::uint32_t h0 = 0x6a09e667;
    std::uint32_t h1 = 0xbb67ae85;
    std::uint32_t h2 = 0x3c6ef372;
    std::uint32_t h3 = 0xa54ff53a;
    std::uint32_t h4 = 0x510e527f;
    std::uint32_t h5 = 0x9b05688c;
    std::uint32_t h6 = 0x1f83d9ab;
    std::uint32_t h7 = 0x5be0cd19;

    std::vector<std::uint8_t> buffer(bytes.begin(), bytes.end());
    std::uint64_t bit_length = buffer.size() * 8;
    buffer.push_back(0x80);
    while (buffer.size() % 64 != 56) buffer.push_back(0x00);
    for (int i = 7; i >= 0; --i) {
        buffer.push_back(static_cast<std::uint8_t>((bit_length >> (i * 8)) & 0xff));
    }

    for (std::size_t chunk = 0; chunk < buffer.size(); chunk += 64) {
        std::uint32_t w[64];
        for (int i = 0; i < 16; ++i) {
            w[i] = (std::uint32_t(buffer[chunk + i * 4]) << 24) |
                   (std::uint32_t(buffer[chunk + i * 4 + 1]) << 16) |
                   (std::uint32_t(buffer[chunk + i * 4 + 2]) << 8) |
                   std::uint32_t(buffer[chunk + i * 4 + 3]);
        }
        for (int i = 16; i < 64; ++i) {
            std::uint32_t s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
            std::uint32_t s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16] + s0 + w[i - 7] + s1;
        }
        std::uint32_t a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, hh = h7;
        for (int i = 0; i < 64; ++i) {
            std::uint32_t S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
            std::uint32_t ch = (e & f) ^ (~e & g);
            std::uint32_t temp1 = hh + S1 + ch + k[i] + w[i];
            std::uint32_t S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
            std::uint32_t maj = (a & b) ^ (a & c) ^ (b & c);
            std::uint32_t temp2 = S0 + maj;
            hh = g; g = f; f = e; e = d + temp1; d = c; c = b; b = a; a = temp1 + temp2;
        }
        h0 += a; h1 += b; h2 += c; h3 += d; h4 += e; h5 += f; h6 += g; h7 += hh;
    }

    std::ostringstream out;
    auto emit = [&](std::uint32_t word) {
        char buf[16];
        std::snprintf(buf, sizeof(buf), "%08x", word);
        out << buf;
    };
    emit(h0); emit(h1); emit(h2); emit(h3);
    emit(h4); emit(h5); emit(h6); emit(h7);
    return out.str();
}

bool write_brep(const TopoDS_Shape& shape, const std::filesystem::path& path, std::string& error) {
    if (!BRepTools::Write(shape, path.string().c_str())) {
        error = "BRepTools::Write failed for " + path.string();
        return false;
    }
    return true;
}

bool analyze_brep(const TopoDS_Shape& shape) {
    BRepCheck_Analyzer analyzer(shape);
    analyzer.SetParallel(false);
    return analyzer.IsValid() != 0;
}

std::string error_response(const std::string& request_id, const std::string& operation,
                           const std::string& feature_id, const std::string& status,
                           const std::string& message) {
    std::ostringstream out;
    out << "{"
        << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
        << "\"request_id\":\"" << json_escape(request_id) << "\","
        << "\"operation\":\"" << json_escape(operation) << "\","
        << "\"status\":\"" << json_escape(status) << "\","
        << "\"diagnostic\":\"" << json_escape(message) << "\","
        << "\"feature_id\":\"" << json_escape(feature_id) << "\""
        << "}";
    return out.str();
}

bool handle_extrude(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    double height = get_number(request, "height");
    auto profile = get_profile(request, "profile");

    if (request_id.empty() || feature_id.empty() || output_dir.empty() || output_filename.empty()) {
        error = "extrude request is missing required string fields";
        return false;
    }
    if (profile.size() < 3) {
        error = "extrude profile must contain at least 3 vertices";
        return false;
    }
    if (!(height > 0.0) || !std::isfinite(height)) {
        error = "extrude height must be a positive finite number";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }

    BRepBuilderAPI_MakePolygon polygon;
    for (const auto& vertex : profile) {
        polygon.Add(gp_Pnt(vertex[0], vertex[1], 0.0));
    }
    polygon.Close();
    if (!polygon.IsDone()) {
        error = "could not build the 2D polygon (non-convex or self-intersecting profile?)";
        return false;
    }
    TopoDS_Wire wire = polygon.Wire();
    BRepBuilderAPI_MakeFace face(wire);
    if (!face.IsDone()) {
        error = "could not build the planar face from the polygon";
        return false;
    }
    TopoDS_Face planar_face = face.Face();
    gp_Vec prism_vector(0.0, 0.0, height);
    BRepPrimAPI_MakePrism prism(planar_face, prism_vector);
    if (!prism.IsDone()) {
        error = "could not prism the face to produce the solid";
        return false;
    }
    TopoDS_Shape solid = prism.Shape();

    std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
    if (output_path.has_parent_path()) {
        std::error_code ec;
        std::filesystem::create_directories(output_path.parent_path(), ec);
    }
    if (!write_brep(solid, output_path, error)) {
        return false;
    }
    std::ifstream stream(output_path, std::ios::binary);
    std::ostringstream bytes;
    bytes << stream.rdbuf();
    std::string sha = sha256_hex(bytes.str());

    std::string status = "ok";
    if (!analyze_brep(solid)) {
        // Seed the error string with the brep_invalid marker so the
        // main-loop exit-code classifier routes this through the
        // brep_invalid diagnostic path (exit code 3) instead of
        // request_malformed.
        error = "brep_invalid: BRepCheck_Analyzer failed";
        status = "brep_invalid";
    }

    std::ostringstream out;
    out << "{"
        << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
        << "\"request_id\":\"" << json_escape(request_id) << "\","
        << "\"operation\":\"extrude\","
        << "\"status\":\"" << json_escape(status) << "\","
        << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
        << "\"brep_sha256\":\"" << json_escape(sha) << "\","
        << "\"brep_bytes\":" << bytes.str().size() << ","
        << "\"feature_id\":\"" << json_escape(feature_id) << "\""
        << "}";
    write_stdout_line(out.str());
    return status == "ok";
}

bool handle_boolean_fuse(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string base_path_str = get_string(request, "base_path");
    std::string tool_path_str = get_string(request, "tool_path");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");

    if (request_id.empty() || feature_id.empty() || base_path_str.empty() ||
        tool_path_str.empty() || output_dir.empty() || output_filename.empty()) {
        error = "boolean_fuse request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }

    TopoDS_Shape base;
    TopoDS_Shape tool;
    BRep_Builder builder;
    if (!BRepTools::Read(base, base_path_str.c_str(), builder)) {
        error = "could not read base BREP at " + base_path_str;
        return false;
    }
    if (!BRepTools::Read(tool, tool_path_str.c_str(), builder)) {
        error = "could not read tool BREP at " + tool_path_str;
        return false;
    }
    if (base.IsNull() || tool.IsNull()) {
        error = "BREP file produced a null TopoDS_Shape";
        return false;
    }

    BRepAlgoAPI_Fuse fuse(base, tool);
    fuse.SetFuzzyValue(1.0e-6);
    fuse.Build();
    if (!fuse.IsDone()) {
        error = "BRepAlgoAPI_Fuse did not complete";
        return false;
    }
    TopoDS_Shape fused = fuse.Shape();

    std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
    if (output_path.has_parent_path()) {
        std::error_code ec;
        std::filesystem::create_directories(output_path.parent_path(), ec);
    }
    if (!write_brep(fused, output_path, error)) {
        return false;
    }
    std::ifstream stream(output_path, std::ios::binary);
    std::ostringstream bytes;
    bytes << stream.rdbuf();
    std::string sha = sha256_hex(bytes.str());

    std::string status = "ok";
    if (!analyze_brep(fused)) {
        error = "brep_invalid: BRepCheck_Analyzer failed";
        status = "brep_invalid";
    }

    std::ostringstream out;
    out << "{"
        << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
        << "\"request_id\":\"" << json_escape(request_id) << "\","
        << "\"operation\":\"boolean_fuse\","
        << "\"status\":\"" << json_escape(status) << "\","
        << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
        << "\"brep_sha256\":\"" << json_escape(sha) << "\","
        << "\"brep_bytes\":" << bytes.str().size() << ","
        << "\"feature_id\":\"" << json_escape(feature_id) << "\""
        << "}";
    write_stdout_line(out.str());
    return status == "ok";
}

bool handle_fillet(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string base_path_str = get_string(request, "base_path");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    double radius = get_number(request, "radius");

    if (request_id.empty() || feature_id.empty() || base_path_str.empty() ||
        output_dir.empty() || output_filename.empty()) {
        error = "fillet request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }
    if (!(radius > 0.0) || !std::isfinite(radius)) {
        error = "fillet radius must be a positive finite number";
        return false;
    }

    try {
        TopoDS_Shape base;
        TopoDS_Shape result;
        BRep_Builder builder;
        if (!BRepTools::Read(base, base_path_str.c_str(), builder)) {
            error = "could not read base BREP at " + base_path_str;
            return false;
        }
        if (base.IsNull()) {
            error = "BREP file produced a null TopoDS_Shape";
            return false;
        }

        BRepFilletAPI_MakeFillet fillet(base);
        for (TopExp_Explorer edge_explorer(base, TopAbs_EDGE); edge_explorer.More(); edge_explorer.Next()) {
            TopoDS_Edge edge = TopoDS::Edge(edge_explorer.Current());
            fillet.Add(radius, edge);
        }
        fillet.Build();
        if (!fillet.IsDone()) {
            error = "BRepFilletAPI_MakeFillet did not complete";
            return false;
        }
        result = fillet.Shape();

        std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
        if (output_path.has_parent_path()) {
            std::error_code ec;
            std::filesystem::create_directories(output_path.parent_path(), ec);
        }
        if (!write_brep(result, output_path, error)) {
            return false;
        }
        std::ifstream stream(output_path, std::ios::binary);
        std::ostringstream bytes;
        bytes << stream.rdbuf();
        std::string sha = sha256_hex(bytes.str());

        std::string status = "ok";
        if (!analyze_brep(result)) {
            error = "brep_invalid: BRepCheck_Analyzer failed";
            status = "brep_invalid";
        }

        std::ostringstream out;
        out << "{"
            << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
            << "\"request_id\":\"" << json_escape(request_id) << "\","
            << "\"operation\":\"fillet\","
            << "\"status\":\"" << json_escape(status) << "\","
            << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
            << "\"brep_sha256\":\"" << json_escape(sha) << "\","
            << "\"brep_bytes\":" << bytes.str().size() << ","
            << "\"feature_id\":\"" << json_escape(feature_id) << "\""
            << "}";
        write_stdout_line(out.str());
        return status == "ok";
    } catch (const Standard_Failure& e) {
        error = "OCCT exception during fillet: ";
        error += e.GetMessageString();
        return false;
    } catch (const std::exception& e) {
        error = "std::exception during fillet: ";
        error += e.what();
        return false;
    }
}

bool handle_chamfer(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string base_path_str = get_string(request, "base_path");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    double distance = get_number(request, "distance");

    if (request_id.empty() || feature_id.empty() || base_path_str.empty() ||
        output_dir.empty() || output_filename.empty()) {
        error = "chamfer request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }
    if (!(distance > 0.0) || !std::isfinite(distance)) {
        error = "chamfer distance must be a positive finite number";
        return false;
    }

    try {
        TopoDS_Shape base;
        TopoDS_Shape result;
        BRep_Builder builder;
        if (!BRepTools::Read(base, base_path_str.c_str(), builder)) {
            error = "could not read base BREP at " + base_path_str;
            return false;
        }
        if (base.IsNull()) {
            error = "BREP file produced a null TopoDS_Shape";
            return false;
        }

        try {
            BRepFilletAPI_MakeChamfer chamfer(base);
            for (TopExp_Explorer edge_explorer(base, TopAbs_EDGE); edge_explorer.More(); edge_explorer.Next()) {
                TopoDS_Edge edge = TopoDS::Edge(edge_explorer.Current());
                chamfer.Add(distance, edge);
            }
            chamfer.Build();
            if (!chamfer.IsDone()) {
                error = "unsupported_geometry: BRepFilletAPI_MakeChamfer did not complete";
                return false;
            }
            result = chamfer.Shape();
        } catch (const Standard_Failure& e) {
            error = "unsupported_geometry: OCCT exception during chamfer: ";
            error += e.GetMessageString();
            return false;
        } catch (const std::exception& e) {
            error = "unsupported_geometry: std::exception during chamfer: ";
            error += e.what();
            return false;
        }

        std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
        if (output_path.has_parent_path()) {
            std::error_code ec;
            std::filesystem::create_directories(output_path.parent_path(), ec);
        }
        if (!write_brep(result, output_path, error)) {
            return false;
        }
        std::ifstream stream(output_path, std::ios::binary);
        std::ostringstream bytes;
        bytes << stream.rdbuf();
        std::string sha = sha256_hex(bytes.str());

        std::string status = "ok";
        if (!analyze_brep(result)) {
            error = "brep_invalid: BRepCheck_Analyzer failed";
            status = "brep_invalid";
        }

        std::ostringstream out;
        out << "{"
            << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
            << "\"request_id\":\"" << json_escape(request_id) << "\","
            << "\"operation\":\"chamfer\","
            << "\"status\":\"" << json_escape(status) << "\","
            << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
            << "\"brep_sha256\":\"" << json_escape(sha) << "\","
            << "\"brep_bytes\":" << bytes.str().size() << ","
            << "\"feature_id\":\"" << json_escape(feature_id) << "\""
            << "}";
        write_stdout_line(out.str());
        return status == "ok";
    } catch (const Standard_Failure& e) {
        error = "OCCT exception during chamfer: ";
        error += e.GetMessageString();
        return false;
    } catch (const std::exception& e) {
        error = "std::exception during chamfer: ";
        error += e.what();
        return false;
    }
}

bool handle_hole(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string base_path_str = get_string(request, "base_path");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    auto position = get_vec3(request, "position");
    auto direction = get_vec3(request, "direction");
    double diameter = get_number(request, "diameter");

    if (request_id.empty() || feature_id.empty() || base_path_str.empty() ||
        output_dir.empty() || output_filename.empty()) {
        error = "hole request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }
    if (!(diameter > 0.0) || !std::isfinite(diameter)) {
        error = "hole diameter must be a positive finite number";
        return false;
    }
    for (double component : position) {
        if (!std::isfinite(component)) {
            error = "hole position components must be finite";
            return false;
        }
    }
    for (double component : direction) {
        if (!std::isfinite(component)) {
            error = "hole direction components must be finite";
            return false;
        }
    }
    double direction_norm_squared = direction[0] * direction[0] +
                                    direction[1] * direction[1] +
                                    direction[2] * direction[2];
    if (direction_norm_squared == 0.0) {
        error = "hole direction must be a non-zero vector";
        return false;
    }

    try {
        TopoDS_Shape base;
        BRep_Builder builder;
        if (!BRepTools::Read(base, base_path_str.c_str(), builder)) {
            error = "could not read base BREP at " + base_path_str;
            return false;
        }
        if (base.IsNull()) {
            error = "BREP file produced a null TopoDS_Shape";
            return false;
        }

        // Span the base bounding box from either side of the requested
        // point, independent of the base's world-space location.
        Bnd_Box bbox;
        BRepBndLib::Add(base, bbox);
        double xmin, ymin, zmin, xmax, ymax, zmax;
        bbox.Get(xmin, ymin, zmin, xmax, ymax, zmax);
        double diagonal = std::sqrt(
            (xmax - xmin) * (xmax - xmin) +
            (ymax - ymin) * (ymax - ymin) +
            (zmax - zmin) * (zmax - zmin));
        if (!(diagonal > 0.0) || !std::isfinite(diagonal)) {
            error = "base BREP bounding box must have finite non-zero extents";
            return false;
        }
        double cylinder_length = 2.0 * diagonal;

        double norm = std::sqrt(direction_norm_squared);
        gp_Dir axis_dir(direction[0] / norm, direction[1] / norm, direction[2] / norm);
        gp_Pnt cutter_start(
            position[0] - axis_dir.X() * diagonal,
            position[1] - axis_dir.Y() * diagonal,
            position[2] - axis_dir.Z() * diagonal);
        // gp_Ax2 needs a "X direction" — the cylinder's reference axis.
        // Pick the first world axis not parallel to the hole axis so the
        // resulting cylinder is unambiguously oriented.
        gp_Dir x_dir;
        if (std::abs(axis_dir.Z()) < 0.9) {
            x_dir = gp::DX();
        } else {
            x_dir = gp::DY();
        }
        gp_Ax2 cylinder_axis(cutter_start, axis_dir, x_dir);
        BRepPrimAPI_MakeCylinder cylinder(cylinder_axis, diameter / 2.0, cylinder_length);
        cylinder.Build();
        if (!cylinder.IsDone()) {
            error = "BRepPrimAPI_MakeCylinder did not complete";
            return false;
        }
        TopoDS_Shape tool = cylinder.Shape();

        BRepAlgoAPI_Cut cut(base, tool);
        cut.SetFuzzyValue(1.0e-6);
        cut.Build();
        if (!cut.IsDone()) {
            error = "BRepAlgoAPI_Cut did not complete";
            return false;
        }
        TopoDS_Shape result = cut.Shape();

        std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
        if (output_path.has_parent_path()) {
            std::error_code ec;
            std::filesystem::create_directories(output_path.parent_path(), ec);
        }
        if (!write_brep(result, output_path, error)) {
            return false;
        }
        TopoDS_Shape serialized_result;
        BRep_Builder output_builder;
        if (!BRepTools::Read(serialized_result, output_path.c_str(), output_builder) ||
            serialized_result.IsNull()) {
            error = "could not read written hole BREP at " + output_path.string();
            return false;
        }
        GProp_GProps base_properties;
        GProp_GProps result_properties;
        BRepGProp::VolumeProperties(base, base_properties);
        BRepGProp::VolumeProperties(serialized_result, result_properties);
        double removed_volume = base_properties.Mass() - result_properties.Mass();
        std::ifstream stream(output_path, std::ios::binary);
        std::ostringstream bytes;
        bytes << stream.rdbuf();
        std::string sha = sha256_hex(bytes.str());

        std::string status = "ok";
        if (!analyze_brep(serialized_result)) {
            error = "brep_invalid: BRepCheck_Analyzer failed";
            status = "brep_invalid";
        }

        std::ostringstream out;
        out << "{"
            << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
            << "\"request_id\":\"" << json_escape(request_id) << "\","
            << "\"operation\":\"hole\","
            << "\"status\":\"" << json_escape(status) << "\","
            << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
            << "\"brep_sha256\":\"" << json_escape(sha) << "\","
            << "\"brep_bytes\":" << bytes.str().size() << ","
            << "\"feature_id\":\"" << json_escape(feature_id) << "\","
            << "\"removed_volume\":" << removed_volume
            << "}";
        write_stdout_line(out.str());
        return status == "ok";
    } catch (const Standard_Failure& e) {
        error = "OCCT exception during hole: ";
        error += e.GetMessageString();
        return false;
    } catch (const std::exception& e) {
        error = "std::exception during hole: ";
        error += e.what();
        return false;
    }
}

bool handle_mirror(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string base_path_str = get_string(request, "base_path");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    auto plane_point = get_vec3(request, "plane_point");
    auto plane_normal = get_vec3(request, "plane_normal");

    if (request_id.empty() || feature_id.empty() || base_path_str.empty() ||
        output_dir.empty() || output_filename.empty()) {
        error = "mirror request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }
    for (double component : plane_point) {
        if (!std::isfinite(component)) {
            error = "mirror plane_point components must be finite";
            return false;
        }
    }
    for (double component : plane_normal) {
        if (!std::isfinite(component)) {
            error = "mirror plane_normal components must be finite";
            return false;
        }
    }
    double normal_norm_squared = plane_normal[0] * plane_normal[0] +
                                  plane_normal[1] * plane_normal[1] +
                                  plane_normal[2] * plane_normal[2];
    if (normal_norm_squared == 0.0) {
        error = "mirror plane_normal must be a non-zero vector";
        return false;
    }

    try {
        TopoDS_Shape base;
        BRep_Builder builder;
        if (!BRepTools::Read(base, base_path_str.c_str(), builder)) {
            error = "could not read base BREP at " + base_path_str;
            return false;
        }
        if (base.IsNull()) {
            error = "BREP file produced a null TopoDS_Shape";
            return false;
        }

        double norm = std::sqrt(normal_norm_squared);
        gp_Pnt origin(plane_point[0], plane_point[1], plane_point[2]);
        gp_Dir normal(plane_normal[0] / norm,
                      plane_normal[1] / norm,
                      plane_normal[2] / norm);
        // gp_Ax2 requires a "X direction" reference axis that is
        // NOT parallel to the plane normal (otherwise the cross
        // product the constructor uses internally collapses to a
        // zero-norm vector). Pick the first world axis whose dot
        // product with the normal is not ~1.
        gp_Dir x_dir;
        if (std::abs(normal.X()) < 0.9) {
            x_dir = gp::DX();
        } else if (std::abs(normal.Y()) < 0.9) {
            x_dir = gp::DY();
        } else {
            x_dir = gp::DZ();
        }
        gp_Ax2 mirror_plane(origin, normal, x_dir);

        gp_Trsf transform;
        transform.SetMirror(mirror_plane);

        BRepBuilderAPI_Transform mirror_op(base, transform, Standard_False, Standard_False);
        mirror_op.Build();
        if (!mirror_op.IsDone()) {
            error = "BRepBuilderAPI_Transform did not complete";
            return false;
        }
        TopoDS_Shape result = mirror_op.Shape();

        std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
        if (output_path.has_parent_path()) {
            std::error_code ec;
            std::filesystem::create_directories(output_path.parent_path(), ec);
        }
        if (!write_brep(result, output_path, error)) {
            return false;
        }
        std::ifstream stream(output_path, std::ios::binary);
        std::ostringstream bytes;
        bytes << stream.rdbuf();
        std::string sha = sha256_hex(bytes.str());

        std::string status = "ok";
        if (!analyze_brep(result)) {
            error = "brep_invalid: BRepCheck_Analyzer failed";
            status = "brep_invalid";
        }

        std::ostringstream out;
        out << "{"
            << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
            << "\"request_id\":\"" << json_escape(request_id) << "\","
            << "\"operation\":\"mirror\","
            << "\"status\":\"" << json_escape(status) << "\","
            << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
            << "\"brep_sha256\":\"" << json_escape(sha) << "\","
            << "\"brep_bytes\":" << bytes.str().size() << ","
            << "\"feature_id\":\"" << json_escape(feature_id) << "\""
            << "}";
        write_stdout_line(out.str());
        return status == "ok";
    } catch (const Standard_Failure& e) {
        error = "OCCT exception during mirror: ";
        error += e.GetMessageString();
        return false;
    } catch (const std::exception& e) {
        error = "std::exception during mirror: ";
        error += e.what();
        return false;
    }
}

bool handle_revolve(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    auto profile = get_profile(request, "profile");
    auto axis_point = get_vec3(request, "axis_point");
    auto axis_direction = get_vec3(request, "axis_direction");
    double angle = get_number(request, "angle");

    if (request_id.empty() || feature_id.empty() || output_dir.empty() || output_filename.empty()) {
        error = "revolve request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }
    if (profile.size() < 3) {
        error = "revolve profile must contain at least 3 vertices";
        return false;
    }
    for (double component : axis_point) {
        if (!std::isfinite(component)) {
            error = "revolve axis_point components must be finite";
            return false;
        }
    }
    for (double component : axis_direction) {
        if (!std::isfinite(component)) {
            error = "revolve axis_direction components must be finite";
            return false;
        }
    }
    double direction_norm_squared = axis_direction[0] * axis_direction[0] +
                                    axis_direction[1] * axis_direction[1] +
                                    axis_direction[2] * axis_direction[2];
    if (direction_norm_squared == 0.0) {
        error = "revolve axis_direction must be a non-zero vector";
        return false;
    }
    if (!(angle > 0.0) || !std::isfinite(angle)) {
        error = "revolve angle must be a positive finite number";
        return false;
    }

    try {
        BRepBuilderAPI_MakePolygon polygon;
        for (const auto& vertex : profile) {
            polygon.Add(gp_Pnt(vertex[0], vertex[1], 0.0));
        }
        polygon.Close();
        if (!polygon.IsDone()) {
            error = "could not build the 2D polygon (non-convex or self-intersecting profile?)";
            return false;
        }
        TopoDS_Wire wire = polygon.Wire();
        BRepBuilderAPI_MakeFace face(wire);
        if (!face.IsDone()) {
            error = "could not build the planar face from the polygon";
            return false;
        }
        TopoDS_Face planar_face = face.Face();

        double norm = std::sqrt(direction_norm_squared);
        gp_Pnt origin(axis_point[0], axis_point[1], axis_point[2]);
        gp_Dir axis_dir(axis_direction[0] / norm,
                        axis_direction[1] / norm,
                        axis_direction[2] / norm);
        gp_Ax1 axis(origin, axis_dir);

        BRepPrimAPI_MakeRevol revol(planar_face, axis, angle);
        revol.Build();
        if (!revol.IsDone()) {
            error = "BRepPrimAPI_MakeRevol did not complete";
            return false;
        }
        TopoDS_Shape result = revol.Shape();

        std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
        if (output_path.has_parent_path()) {
            std::error_code ec;
            std::filesystem::create_directories(output_path.parent_path(), ec);
        }
        if (!write_brep(result, output_path, error)) {
            return false;
        }
        std::ifstream stream(output_path, std::ios::binary);
        std::ostringstream bytes;
        bytes << stream.rdbuf();
        std::string sha = sha256_hex(bytes.str());

        std::string status = "ok";
        if (!analyze_brep(result)) {
            error = "brep_invalid: BRepCheck_Analyzer failed";
            status = "brep_invalid";
        }

        std::ostringstream out;
        out << "{"
            << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
            << "\"request_id\":\"" << json_escape(request_id) << "\","
            << "\"operation\":\"revolve\","
            << "\"status\":\"" << json_escape(status) << "\","
            << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
            << "\"brep_sha256\":\"" << json_escape(sha) << "\","
            << "\"brep_bytes\":" << bytes.str().size() << ","
            << "\"feature_id\":\"" << json_escape(feature_id) << "\""
            << "}";
        write_stdout_line(out.str());
        return status == "ok";
    } catch (const Standard_Failure& e) {
        error = "OCCT exception during revolve: ";
        error += e.GetMessageString();
        return false;
    } catch (const std::exception& e) {
        error = "std::exception during revolve: ";
        error += e.what();
        return false;
    }
}

bool handle_linear_pattern(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string base_path_str = get_string(request, "base_path");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    auto direction = get_vec3(request, "direction");
    double count_value = get_number(request, "count");
    double spacing = get_number(request, "spacing");

    if (request_id.empty() || feature_id.empty() || base_path_str.empty() ||
        output_dir.empty() || output_filename.empty()) {
        error = "linear_pattern request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }
    for (double component : direction) {
        if (!std::isfinite(component)) {
            error = "linear_pattern direction components must be finite";
            return false;
        }
    }
    double direction_norm_squared = direction[0] * direction[0] +
                                    direction[1] * direction[1] +
                                    direction[2] * direction[2];
    if (direction_norm_squared == 0.0) {
        error = "linear_pattern direction must be a non-zero vector";
        return false;
    }
    if (!std::isfinite(count_value)) {
        error = "linear_pattern count must be a finite number";
        return false;
    }
    std::uint32_t count = static_cast<std::uint32_t>(count_value);
    if (static_cast<double>(count) != count_value || count < 1) {
        error = "linear_pattern count must be an integer >= 1";
        return false;
    }
    if (!(spacing > 0.0) || !std::isfinite(spacing)) {
        error = "linear_pattern spacing must be a positive finite number";
        return false;
    }

    try {
        TopoDS_Shape base;
        BRep_Builder builder;
        if (!BRepTools::Read(base, base_path_str.c_str(), builder)) {
            error = "could not read base BREP at " + base_path_str;
            return false;
        }
        if (base.IsNull()) {
            error = "BREP file produced a null TopoDS_Shape";
            return false;
        }

        double norm = std::sqrt(direction_norm_squared);
        gp_Vec step(direction[0] / norm * spacing,
                    direction[1] / norm * spacing,
                    direction[2] / norm * spacing);

        TopoDS_Shape result = base;
        for (std::uint32_t index = 1; index < count; ++index) {
            gp_Trsf transform;
            transform.SetTranslation(step * static_cast<double>(index));
            BRepBuilderAPI_Transform translated(base, transform, Standard_False, Standard_False);
            translated.Build();
            if (!translated.IsDone()) {
                error = "BRepBuilderAPI_Transform did not complete during linear pattern copy";
                return false;
            }
            TopoDS_Shape copy = translated.Shape();
            BRepAlgoAPI_Fuse fuse(result, copy);
            fuse.SetFuzzyValue(1.0e-6);
            fuse.Build();
            if (!fuse.IsDone()) {
                error = "BRepAlgoAPI_Fuse did not complete during linear pattern copy";
                return false;
            }
            result = fuse.Shape();
        }

        std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
        if (output_path.has_parent_path()) {
            std::error_code ec;
            std::filesystem::create_directories(output_path.parent_path(), ec);
        }
        if (!write_brep(result, output_path, error)) {
            return false;
        }
        std::ifstream stream(output_path, std::ios::binary);
        std::ostringstream bytes;
        bytes << stream.rdbuf();
        std::string sha = sha256_hex(bytes.str());

        std::string status = "ok";
        if (!analyze_brep(result)) {
            error = "brep_invalid: BRepCheck_Analyzer failed";
            status = "brep_invalid";
        }

        std::ostringstream out;
        out << "{"
            << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
            << "\"request_id\":\"" << json_escape(request_id) << "\","
            << "\"operation\":\"linear_pattern\","
            << "\"status\":\"" << json_escape(status) << "\","
            << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
            << "\"brep_sha256\":\"" << json_escape(sha) << "\","
            << "\"brep_bytes\":" << bytes.str().size() << ","
            << "\"feature_id\":\"" << json_escape(feature_id) << "\""
            << "}";
        write_stdout_line(out.str());
        return status == "ok";
    } catch (const Standard_Failure& e) {
        error = "OCCT exception during linear_pattern: ";
        error += e.GetMessageString();
        return false;
    } catch (const std::exception& e) {
        error = "std::exception during linear_pattern: ";
        error += e.what();
        return false;
    }
}

bool handle_circular_pattern(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string base_path_str = get_string(request, "base_path");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    auto axis_point = get_vec3(request, "axis_point");
    auto axis_normal = get_vec3(request, "axis_normal");
    double angle_step = get_number(request, "angle_step");
    double count_value = get_number(request, "count");

    if (request_id.empty() || feature_id.empty() || base_path_str.empty() ||
        output_dir.empty() || output_filename.empty()) {
        error = "circular_pattern request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }
    for (double component : axis_point) {
        if (!std::isfinite(component)) {
            error = "circular_pattern axis_point components must be finite";
            return false;
        }
    }
    for (double component : axis_normal) {
        if (!std::isfinite(component)) {
            error = "circular_pattern axis_normal components must be finite";
            return false;
        }
    }
    double normal_norm_squared = axis_normal[0] * axis_normal[0] +
                                  axis_normal[1] * axis_normal[1] +
                                  axis_normal[2] * axis_normal[2];
    if (normal_norm_squared == 0.0) {
        error = "circular_pattern axis_normal must be a non-zero vector";
        return false;
    }
    if (!std::isfinite(angle_step) || !(angle_step > 0.0) || angle_step > 2.0 * M_PI) {
        error = "circular_pattern angle_step must be a positive finite number <= 2π";
        return false;
    }
    if (!std::isfinite(count_value)) {
        error = "circular_pattern count must be a finite number";
        return false;
    }
    std::uint32_t count = static_cast<std::uint32_t>(count_value);
    if (static_cast<double>(count) != count_value || count < 1) {
        error = "circular_pattern count must be an integer >= 1";
        return false;
    }

    try {
        TopoDS_Shape base;
        BRep_Builder builder;
        if (!BRepTools::Read(base, base_path_str.c_str(), builder)) {
            error = "could not read base BREP at " + base_path_str;
            return false;
        }
        if (base.IsNull()) {
            error = "BREP file produced a null TopoDS_Shape";
            return false;
        }

        double norm = std::sqrt(normal_norm_squared);
        gp_Pnt origin(axis_point[0], axis_point[1], axis_point[2]);
        gp_Dir normal(axis_normal[0] / norm,
                      axis_normal[1] / norm,
                      axis_normal[2] / norm);
        gp_Ax1 rotation_axis(origin, normal);

        TopoDS_Shape result = base;
        for (std::uint32_t index = 1; index < count; ++index) {
            gp_Trsf transform;
            transform.SetRotation(rotation_axis, angle_step * static_cast<double>(index));
            BRepBuilderAPI_Transform rotated(base, transform, Standard_False, Standard_False);
            rotated.Build();
            if (!rotated.IsDone()) {
                error = "BRepBuilderAPI_Transform did not complete during circular pattern copy";
                return false;
            }
            TopoDS_Shape copy = rotated.Shape();
            BRepAlgoAPI_Fuse fuse(result, copy);
            fuse.SetFuzzyValue(1.0e-6);
            fuse.Build();
            if (!fuse.IsDone()) {
                error = "BRepAlgoAPI_Fuse did not complete during circular pattern copy";
                return false;
            }
            result = fuse.Shape();
        }

        std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
        if (output_path.has_parent_path()) {
            std::error_code ec;
            std::filesystem::create_directories(output_path.parent_path(), ec);
        }
        if (!write_brep(result, output_path, error)) {
            return false;
        }
        std::ifstream stream(output_path, std::ios::binary);
        std::ostringstream bytes;
        bytes << stream.rdbuf();
        std::string sha = sha256_hex(bytes.str());

        std::string status = "ok";
        if (!analyze_brep(result)) {
            error = "brep_invalid: BRepCheck_Analyzer failed";
            status = "brep_invalid";
        }

        std::ostringstream out;
        out << "{"
            << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
            << "\"request_id\":\"" << json_escape(request_id) << "\","
            << "\"operation\":\"circular_pattern\","
            << "\"status\":\"" << json_escape(status) << "\","
            << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
            << "\"brep_sha256\":\"" << json_escape(sha) << "\","
            << "\"brep_bytes\":" << bytes.str().size() << ","
            << "\"feature_id\":\"" << json_escape(feature_id) << "\""
            << "}";
        write_stdout_line(out.str());
        return status == "ok";
    } catch (const Standard_Failure& e) {
        error = "OCCT exception during circular_pattern: ";
        error += e.GetMessageString();
        return false;
    } catch (const std::exception& e) {
        error = "std::exception during circular_pattern: ";
        error += e.what();
        return false;
    }
}

bool handle_shell(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string base_path_str = get_string(request, "base_path");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    double thickness = get_number(request, "thickness");

    if (request_id.empty() || feature_id.empty() || base_path_str.empty() ||
        output_dir.empty() || output_filename.empty()) {
        error = "shell request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }
    if (!std::isfinite(thickness) || !(thickness > 0.0)) {
        error = "shell thickness must be a positive finite number";
        return false;
    }

    try {
        TopoDS_Shape base;
        BRep_Builder builder;
        if (!BRepTools::Read(base, base_path_str.c_str(), builder)) {
            error = "could not read base BREP at " + base_path_str;
            return false;
        }
        if (base.IsNull()) {
            error = "BREP file produced a null TopoDS_Shape";
            return false;
        }
        // `BRepOffsetAPI_MakeThickSolid::MakeThickSolidByJoin` expects a
        // single `TopoDS_Solid`. Boolean-fused inputs can come back as
        // a COMPSOLID (touching solids) or a COMPOUND; pick the first
        // inner solid so the offset algorithm has a single body to
        // shell.
        TopoDS_Solid base_solid;
        if (base.ShapeType() == TopAbs_SOLID) {
            base_solid = TopoDS::Solid(base);
        } else if (base.ShapeType() == TopAbs_COMPSOLID ||
                   base.ShapeType() == TopAbs_COMPOUND) {
            for (TopExp_Explorer ex(base, TopAbs_SOLID); ex.More();
                 ex.Next()) {
                base_solid = TopoDS::Solid(ex.Current());
                break;
            }
        }
        if (base_solid.IsNull()) {
            error = "shell base has no TopoDS_Solid";
            return false;
        }

        // `BRepOffsetAPI_MakeThickSolid` requires C1-continuous
        // surfaces and refuses mixed-valence vertices. A BooleanFuse
        // result typically carries internal seams and partial-merge
        // edges that trip the offset algorithm (yielding a null
        // shape). `ShapeUpgrade_UnifySameDomain` merges co-planar
        // faces and smooth-continuous edges before the offset so the
        // algorithm sees a clean shell.
        Handle(ShapeUpgrade_UnifySameDomain) unifier =
            new ShapeUpgrade_UnifySameDomain(base_solid);
        unifier->AllowInternalEdges(Standard_False);
        unifier->Build();
        TopoDS_Shape unified = unifier->Shape();
        if (unified.IsNull()) {
            unified = base_solid;
        }
        if (unified.ShapeType() == TopAbs_SOLID) {
            base_solid = TopoDS::Solid(unified);
        } else {
            for (TopExp_Explorer ex(unified, TopAbs_SOLID); ex.More();
                 ex.Next()) {
                base_solid = TopoDS::Solid(ex.Current());
                break;
            }
        }
        if (base_solid.IsNull()) {
            error = "shell base has no TopoDS_Solid after unification";
            return false;
        }

        // Rebuild the solid from its outer shell so the offset
        // algorithm operates on a closed, single-shell body without
        // residual internal faces from the fuse.
        TopoDS_Shell outer_shell;
        for (TopExp_Explorer ex(base_solid, TopAbs_SHELL); ex.More();
             ex.Next()) {
            outer_shell = TopoDS::Shell(ex.Current());
            break;
        }
        if (outer_shell.IsNull()) {
            error = "shell base has no outer shell";
            return false;
        }
        BRepBuilderAPI_MakeSolid solid_rebuild(outer_shell);
        if (!solid_rebuild.IsDone()) {
            error = "could not rebuild base solid from outer shell";
            return false;
        }
        TopoDS_Solid clean_solid = solid_rebuild.Solid();

        // `MakeThickSolidByJoin` produces the hollow shell directly:
        // the negative offset shrinks every face inward by
        // `thickness` and the closing walls are stitched to the outer
        // shell, yielding a single solid bounded by the original
        // outer surface and an inner offset shell.
        BRepOffsetAPI_MakeThickSolid thickener;
        thickener.MakeThickSolidByJoin(
            clean_solid, TopTools_ListOfShape(),
            -thickness, 1.0e-6,
            BRepOffset_Skin, Standard_False, Standard_False,
            GeomAbs_Arc, Standard_False);
        if (!thickener.IsDone()) {
            error = "BRepOffsetAPI_MakeThickSolid did not complete";
            return false;
        }
        TopoDS_Shape shelled = thickener.Shape();
        if (shelled.IsNull()) {
            error = "BRepOffsetAPI_MakeThickSolid returned a null shape";
            return false;
        }

        std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
        if (output_path.has_parent_path()) {
            std::error_code ec;
            std::filesystem::create_directories(output_path.parent_path(), ec);
        }
        if (!write_brep(shelled, output_path, error)) {
            return false;
        }
        std::ifstream stream(output_path, std::ios::binary);
        std::ostringstream bytes;
        bytes << stream.rdbuf();
        std::string sha = sha256_hex(bytes.str());

        std::string status = "ok";
        if (!analyze_brep(shelled)) {
            error = "brep_invalid: BRepCheck_Analyzer failed";
            status = "brep_invalid";
        }

        std::ostringstream out;
        out << "{"
            << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
            << "\"request_id\":\"" << json_escape(request_id) << "\","
            << "\"operation\":\"shell\","
            << "\"status\":\"" << json_escape(status) << "\","
            << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
            << "\"brep_sha256\":\"" << json_escape(sha) << "\","
            << "\"brep_bytes\":" << bytes.str().size() << ","
            << "\"feature_id\":\"" << json_escape(feature_id) << "\""
            << "}";
        write_stdout_line(out.str());
        return status == "ok";
    } catch (const Standard_Failure& e) {
        error = "OCCT exception during shell: ";
        error += e.GetMessageString();
        return false;
    } catch (const std::exception& e) {
        error = "std::exception during shell: ";
        error += e.what();
        return false;
    }
}

bool handle_draft(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string base_path_str = get_string(request, "base_path");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    double angle = get_number(request, "angle");
    std::array<double, 3> pull_direction = get_vec3(request, "pull_direction");

    if (request_id.empty() || feature_id.empty() || base_path_str.empty() ||
        output_dir.empty() || output_filename.empty()) {
        error = "draft request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }
    if (!std::isfinite(angle) || !(angle > 0.0)) {
        error = "draft angle must be a positive finite number";
        return false;
    }
    double magnitude_squared = 0.0;
    for (double component : pull_direction) {
        if (!std::isfinite(component)) {
            error = "draft pull_direction must contain only finite numbers";
            return false;
        }
        magnitude_squared += component * component;
    }
    if (!(magnitude_squared > 0.0)) {
        error = "draft pull_direction must be a non-zero vector";
        return false;
    }

    try {
        TopoDS_Shape base;
        BRep_Builder builder;
        if (!BRepTools::Read(base, base_path_str.c_str(), builder)) {
            error = "could not read base BREP at " + base_path_str;
            return false;
        }
        if (base.IsNull()) {
            error = "BREP file produced a null TopoDS_Shape";
            return false;
        }
        // The draft algorithm wants a single `TopoDS_Solid`; unwrap a
        // compound or compsolid the same way shell does.
        TopoDS_Solid base_solid;
        if (base.ShapeType() == TopAbs_SOLID) {
            base_solid = TopoDS::Solid(base);
        } else if (base.ShapeType() == TopAbs_COMPSOLID ||
                   base.ShapeType() == TopAbs_COMPOUND) {
            for (TopExp_Explorer ex(base, TopAbs_SOLID); ex.More(); ex.Next()) {
                base_solid = TopoDS::Solid(ex.Current());
                break;
            }
        }
        if (base_solid.IsNull()) {
            error = "draft base has no TopoDS_Solid";
            return false;
        }

        // `BRepOffsetAPI_DraftAngle` requires C1-continuous surfaces and
        // refuses mixed-valence vertices. A BooleanFuse result typically
        // carries internal seams and partial-merge edges that trip the
        // draft algorithm (yielding a null shape or an OCCT exception).
        // `ShapeUpgrade_UnifySameDomain` merges co-planar faces and
        // smooth-continuous edges before the draft so the algorithm
        // sees a clean shell.
        Handle(ShapeUpgrade_UnifySameDomain) unifier =
            new ShapeUpgrade_UnifySameDomain(base_solid);
        unifier->AllowInternalEdges(Standard_False);
        unifier->Build();
        TopoDS_Shape unified = unifier->Shape();
        if (unified.IsNull()) {
            unified = base_solid;
        }
        if (unified.ShapeType() == TopAbs_SOLID) {
            base_solid = TopoDS::Solid(unified);
        } else {
            for (TopExp_Explorer ex(unified, TopAbs_SOLID); ex.More(); ex.Next()) {
                base_solid = TopoDS::Solid(ex.Current());
                break;
            }
        }
        if (base_solid.IsNull()) {
            error = "draft base has no TopoDS_Solid after unification";
            return false;
        }

        gp_Dir pull_dir(pull_direction[0], pull_direction[1], pull_direction[2]);
        // `gp_Dir` normalizes a non-zero input vector; we already
        // rejected the zero vector above.

        // Collect every planar face whose normal is (anti-)parallel to
        // `pull_dir` as a cap. Among the caps, pick the one whose
        // centroid has the most negative dot product with `pull_dir`
        // as the neutral face — that is the cap furthest "behind" the
        // pull direction, i.e. the natural base of the extrusion.
        struct Cap {
            TopoDS_Face face;
            gp_Pnt centroid;
            gp_Dir normal;
        };
        std::vector<Cap> caps;
        for (TopExp_Explorer ex(base_solid, TopAbs_FACE); ex.More(); ex.Next()) {
            TopoDS_Face face = TopoDS::Face(ex.Current());
            Handle(Geom_Surface) surface = BRep_Tool::Surface(face);
            if (surface.IsNull()) continue;
            if (surface->DynamicType() != STANDARD_TYPE(Geom_Plane)) continue;
            Handle(Geom_Plane) plane = Handle(Geom_Plane)::DownCast(surface);
            if (plane.IsNull()) continue;
            gp_Pln pln = plane->Pln();
            gp_Dir face_normal = pln.Axis().Direction();
            // Accept either orientation (face normal points either way).
            if (std::abs(face_normal.X() * pull_dir.X() +
                         face_normal.Y() * pull_dir.Y() +
                         face_normal.Z() * pull_dir.Z()) < 1.0 - 1e-6) {
                continue;
            }
            // Approximate the face centroid by sampling its bounding box.
            Bnd_Box box;
            BRepBndLib::Add(face, box);
            Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
            box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
            gp_Pnt centroid(0.5 * (xmin + xmax), 0.5 * (ymin + ymax),
                            0.5 * (zmin + zmax));
            caps.push_back({face, centroid, face_normal});
        }
        if (caps.empty()) {
            error = "draft base has no planar face parallel to pull_direction";
            return false;
        }
        // Pick the cap with the most negative centroid dot product.
        std::size_t neutral_index = 0;
        double best_projection = std::numeric_limits<double>::infinity();
        for (std::size_t index = 0; index < caps.size(); ++index) {
            const Cap& cap = caps[index];
            double projection = cap.centroid.X() * pull_dir.X() +
                                cap.centroid.Y() * pull_dir.Y() +
                                cap.centroid.Z() * pull_dir.Z();
            if (projection < best_projection) {
                best_projection = projection;
                neutral_index = index;
            }
        }
        const Cap& neutral = caps[neutral_index];
        // `BRepOffsetAPI_DraftAngle::Add` takes a `gp_Dir` that points
        // toward the side material is removed from. With a positive
        // draft angle we want material removed on the far side, so we
        // orient the direction toward the pull direction (positive
        // projection). If the neutral face's stored normal already
        // points along +pull_dir, leave it; otherwise flip it.
        gp_Dir direction_for_draft = neutral.normal;
        if (direction_for_draft.X() * pull_dir.X() +
                direction_for_draft.Y() * pull_dir.Y() +
                direction_for_draft.Z() * pull_dir.Z() <
            0.0) {
            direction_for_draft = gp_Dir(-neutral.normal.X(),
                                         -neutral.normal.Y(),
                                         -neutral.normal.Z());
        }
        gp_Pln neutral_plane(neutral.centroid, direction_for_draft);

        BRepOffsetAPI_DraftAngle draft;
        draft.Init(base_solid);
        // Draft every face that is NOT a planar cap. A face whose
        // normal is parallel to the pull direction is a cap (top/bottom
        // of an extrusion); drafting such a face is undefined because
        // it is parallel to the neutral plane. The neutral cap is
        // excluded above; the remaining caps are excluded here.
        for (TopExp_Explorer ex(base_solid, TopAbs_FACE); ex.More(); ex.Next()) {
            TopoDS_Face face = TopoDS::Face(ex.Current());
            bool is_cap = false;
            for (const Cap& cap : caps) {
                if (cap.face.IsSame(face)) {
                    is_cap = true;
                    break;
                }
            }
            if (is_cap) continue;
            draft.Add(face, direction_for_draft, angle, neutral_plane,
                      Standard_True);
            if (!draft.AddDone()) {
                Draft_ErrorStatus status = draft.Status();
                error = "draft_failed: face status=" +
                        std::to_string(static_cast<int>(status));
                return false;
            }
        }
        draft.Build();
        TopoDS_Shape drafted = draft.Shape();
        if (drafted.IsNull()) {
            error = "BRepOffsetAPI_DraftAngle returned a null shape";
            return false;
        }

        std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
        if (output_path.has_parent_path()) {
            std::error_code ec;
            std::filesystem::create_directories(output_path.parent_path(), ec);
        }
        if (!write_brep(drafted, output_path, error)) {
            return false;
        }
        std::ifstream stream(output_path, std::ios::binary);
        std::ostringstream bytes;
        bytes << stream.rdbuf();
        std::string sha = sha256_hex(bytes.str());

        std::string status = "ok";
        if (!analyze_brep(drafted)) {
            error = "brep_invalid: BRepCheck_Analyzer failed";
            status = "brep_invalid";
        }

        std::ostringstream out;
        out << "{"
            << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
            << "\"request_id\":\"" << json_escape(request_id) << "\","
            << "\"operation\":\"draft\","
            << "\"status\":\"" << json_escape(status) << "\","
            << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
            << "\"brep_sha256\":\"" << json_escape(sha) << "\","
            << "\"brep_bytes\":" << bytes.str().size() << ","
            << "\"feature_id\":\"" << json_escape(feature_id) << "\""
            << "}";
        write_stdout_line(out.str());
        return status == "ok";
    } catch (const Standard_Failure& e) {
        error = "OCCT exception during draft: ";
        error += e.GetMessageString();
        return false;
    } catch (const std::exception& e) {
        error = "std::exception during draft: ";
        error += e.what();
        return false;
    }
}

bool handle_loft(const JsonParser::Value& request, std::string& error) {
    std::string request_id = get_string(request, "request_id");
    std::string feature_id = get_string(request, "feature_id");
    std::string output_dir = get_string(request, "output_dir");
    std::string output_filename = get_string(request, "output_filename");
    std::vector<std::vector<std::array<double, 3>>> profiles;
    if (!get_profiles(request, "profiles", profiles, error)) return false;
    bool is_solid = get_bool(request, "is_solid", true, error);
    if (!error.empty()) return false;
    bool ruled = get_bool(request, "ruled", false, error);
    if (!error.empty()) return false;

    if (request_id.empty() || feature_id.empty() || output_dir.empty() || output_filename.empty()) {
        error = "loft request is missing required string fields";
        return false;
    }
    if (output_filename.find('/') != std::string::npos) {
        error = "output_filename must not contain a path separator";
        return false;
    }
    if (profiles.size() < 2) {
        error = "loft requires at least two profiles";
        return false;
    }
    for (std::size_t index = 0; index < profiles.size(); ++index) {
        if (profiles[index].size() < 3) {
            error = "loft profile " + std::to_string(index) +
                    " must contain at least 3 vertices";
            return false;
        }
        for (const auto& vertex : profiles[index]) {
            for (double component : vertex) {
                if (!std::isfinite(component)) {
                    error = "loft profile " + std::to_string(index) +
                            " contains non-finite coordinates";
                    return false;
                }
            }
        }
    }

    try {
        BRepOffsetAPI_ThruSections loft(is_solid ? Standard_True : Standard_False,
                                        ruled ? Standard_True : Standard_False);
        loft.CheckCompatibility(Standard_True);

        for (std::size_t index = 0; index < profiles.size(); ++index) {
            BRepBuilderAPI_MakePolygon polygon;
            for (const auto& vertex : profiles[index]) {
                polygon.Add(gp_Pnt(vertex[0], vertex[1], vertex[2]));
            }
            polygon.Close();
            if (!polygon.IsDone()) {
                error = "could not build profile " + std::to_string(index) +
                        " (non-convex or self-intersecting?)";
                return false;
            }
            loft.AddWire(polygon.Wire());
        }

        loft.Build();
        if (loft.IsDone() == Standard_False) {
            error = "BRepOffsetAPI_ThruSections did not complete";
            return false;
        }
        TopoDS_Shape shape = loft.Shape();
        if (shape.IsNull()) {
            error = "BRepOffsetAPI_ThruSections returned a null shape";
            return false;
        }

        std::filesystem::path output_path = std::filesystem::path(output_dir) / output_filename;
        if (output_path.has_parent_path()) {
            std::error_code ec;
            std::filesystem::create_directories(output_path.parent_path(), ec);
        }
        if (!write_brep(shape, output_path, error)) {
            return false;
        }
        std::ifstream stream(output_path, std::ios::binary);
        std::ostringstream bytes;
        bytes << stream.rdbuf();
        std::string sha = sha256_hex(bytes.str());

        std::string status = "ok";
        if (!analyze_brep(shape)) {
            error = "brep_invalid: BRepCheck_Analyzer failed";
            status = "brep_invalid";
        }

        std::ostringstream out;
        out << "{"
            << "\"schema_version\":\"" << json_escape(kSchemaVersion) << "\","
            << "\"request_id\":\"" << json_escape(request_id) << "\","
            << "\"operation\":\"loft\","
            << "\"status\":\"" << json_escape(status) << "\","
            << "\"brep_path\":\"" << json_escape(output_path.string()) << "\","
            << "\"brep_sha256\":\"" << json_escape(sha) << "\","
            << "\"brep_bytes\":" << bytes.str().size() << ","
            << "\"feature_id\":\"" << json_escape(feature_id) << "\""
            << "}";
        write_stdout_line(out.str());
        return status == "ok";
    } catch (const Standard_Failure& e) {
        error = "OCCT exception during loft: ";
        error += e.GetMessageString();
        return false;
    } catch (const std::exception& e) {
        error = "std::exception during loft: ";
        error += e.what();
        return false;
    }
}

}  // namespace

int main() {
    std::string raw = read_stdin();
    if (raw.empty()) {
        write_stderr_line("request_malformed: empty stdin");
        return 2;
    }

    JsonParser parser(raw);
    JsonParser::Value envelope;
    std::string error;
    if (!parser.parse_value(&envelope, error)) {
        write_stderr_line("request_malformed: " + error);
        return 2;
    }
    if (envelope.kind != JsonParser::ValueKind::Object) {
        write_stderr_line("request_malformed: top-level must be an object");
        return 2;
    }
    std::string schema_version = get_string(envelope, "schema_version");
    if (schema_version != kSchemaVersion) {
        write_stderr_line("request_malformed: schema_version mismatch (received " +
                          schema_version + ")");
        return 2;
    }
    std::string request_id = get_string(envelope, "request_id");
    std::string operation = get_string(envelope, "operation");
    std::string feature_id = get_string(envelope, "feature_id");

    bool success = false;
    if (operation == "extrude") {
        success = handle_extrude(envelope, error);
    } else if (operation == "boolean_fuse") {
        success = handle_boolean_fuse(envelope, error);
    } else if (operation == "fillet") {
        success = handle_fillet(envelope, error);
    } else if (operation == "chamfer") {
        success = handle_chamfer(envelope, error);
    } else if (operation == "hole") {
        success = handle_hole(envelope, error);
    } else if (operation == "revolve") {
        success = handle_revolve(envelope, error);
    } else if (operation == "mirror") {
        success = handle_mirror(envelope, error);
    } else if (operation == "linear_pattern") {
        success = handle_linear_pattern(envelope, error);
    } else if (operation == "circular_pattern") {
        success = handle_circular_pattern(envelope, error);
    } else if (operation == "shell") {
        success = handle_shell(envelope, error);
    } else if (operation == "draft") {
        success = handle_draft(envelope, error);
    } else if (operation == "loft") {
        success = handle_loft(envelope, error);
    } else {
        write_stderr_line(
            "request_malformed: operation must be extrude, boolean_fuse, fillet, chamfer, \
hole, revolve, mirror, linear_pattern, circular_pattern, shell, draft, or loft");
        return 2;
    }

    if (!success) {
        if (error.empty()) {
            error = "operation returned a non-ok status";
        }
        // The handle_* functions seed `error` with the literal
        // "brep_invalid:" prefix when the BREP fails BRepCheck_Analyzer.
        // A builder rejection identifies geometry that OCCT cannot support.
        bool is_brep_invalid = error.find("brep_invalid:") == 0;
        bool is_unsupported_geometry = error.find("unsupported_geometry:") == 0;
        std::string status = is_brep_invalid ? "brep_invalid"
            : (is_unsupported_geometry ? "unsupported_geometry" : "request_malformed");
        int exit_code = is_brep_invalid ? 3 : (is_unsupported_geometry ? 4 : 2);
        write_stderr_line(error_response(request_id, operation, feature_id, status, error));
        return exit_code;
    }
    return 0;
}
