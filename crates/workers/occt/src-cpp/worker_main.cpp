// SPDX-License-Identifier: LGPL-2.1-or-later
//
// threeterm-occt-worker: disposable worker binary for the ThreeTerm OCCT
// geometry kernel. Reads a JSON envelope from stdin, runs the requested
// operation (extrude or boolean_fuse), validates the BREP with
// BRepCheck_Analyzer, and writes the BREP to the host-staged output path.
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

#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBndLib.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakePolygon.hxx>
#include <BRepBuilderAPI_MakeVertex.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRep_Builder.hxx>
#include <BRepTools.hxx>
#include <Bnd_Box.hxx>
#include <Message_ProgressRange.hxx>
#include <Standard_IStream.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_CompSolid.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Solid.hxx>

#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>

#include <cmath>
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

    BRepFilletAPI_MakeChamfer chamfer(base);
    for (TopExp_Explorer edge_explorer(base, TopAbs_EDGE); edge_explorer.More(); edge_explorer.Next()) {
        TopoDS_Edge edge = TopoDS::Edge(edge_explorer.Current());
        chamfer.Add(distance, edge);
    }
    chamfer.Build();
    if (!chamfer.IsDone()) {
        error = "BRepFilletAPI_MakeChamfer did not complete";
        return false;
    }
    result = chamfer.Shape();

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
    } else {
        write_stderr_line(
            "request_malformed: operation must be extrude, boolean_fuse, fillet, or chamfer");
        return 2;
    }

    if (!success) {
        if (error.empty()) {
            error = "operation returned a non-ok status";
        }
        // The handle_* functions seed `error` with the literal
        // "brep_invalid:" prefix when the BREP fails BRepCheck_Analyzer.
        // Everything else routes through request_malformed.
        bool is_brep_invalid = error.find("brep_invalid:") == 0;
        std::string status = is_brep_invalid ? "brep_invalid" : "request_malformed";
        int exit_code = is_brep_invalid ? 3 : 2;
        write_stderr_line(error_response(request_id, operation, feature_id, status, error));
        return exit_code;
    }
    return 0;
}
