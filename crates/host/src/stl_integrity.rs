//! Read-only, independent validation for exported STL meshes.
//!
//! The verifier deliberately does not share an exporter or geometric-kernel
//! implementation. It parses the bytes it is given, never repairs them, and
//! reports stable failure categories for acceptance tests and conformance
//! tooling.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::fs;
use std::path::Path;

const GEOMETRIC_EPSILON: f64 = 1.0e-10;

/// STL encoding selected by the strict parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StlFormat {
    Ascii,
    Binary,
}

/// Stable reason categories for rejected meshes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityReason {
    Io,
    Empty,
    Format,
    Truncated,
    SizeMismatch,
    NonFinite,
    Degenerate,
    DuplicateFacet,
    OpenBoundary,
    NonManifold,
    InconsistentOrientation,
    MaterialBodyCount,
    CavityContainment,
    NonPositiveVolume,
}

/// A structured, location-aware verifier failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StlIntegrityError {
    pub reason: IntegrityReason,
    pub detail: String,
    pub location: Option<IntegrityLocation>,
}

impl StlIntegrityError {
    fn new(reason: IntegrityReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
            location: None,
        }
    }

    fn at_line(reason: IntegrityReason, line: usize, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
            location: Some(IntegrityLocation {
                line: Some(line),
                ..IntegrityLocation::default()
            }),
        }
    }

    fn at_triangle(reason: IntegrityReason, triangle: usize, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
            location: Some(IntegrityLocation {
                triangle: Some(triangle),
                ..IntegrityLocation::default()
            }),
        }
    }
}

impl fmt::Display for StlIntegrityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.reason, self.detail)
    }
}

impl std::error::Error for StlIntegrityError {}

/// The byte/mesh location associated with a rejection, when one exists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrityLocation {
    pub byte_offset: Option<usize>,
    pub line: Option<usize>,
    pub triangle: Option<usize>,
    pub shell: Option<usize>,
}

/// Summary of a verified STL mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct StlIntegrityReport {
    pub format: StlFormat,
    pub triangle_count: usize,
    pub unique_vertex_count: usize,
    pub shell_count: usize,
    pub cavity_shell_count: usize,
    pub shell_signed_volumes: Vec<f64>,
    pub signed_volume: f64,
    pub material_volume: f64,
}

/// Verify an STL file without changing or repairing it.
pub fn verify_path(path: impl AsRef<Path>) -> Result<StlIntegrityReport, StlIntegrityError> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|error| {
        StlIntegrityError::new(
            IntegrityReason::Io,
            format!("could not read {}: {error}", path.display()),
        )
    })?;
    verify_bytes(&bytes)
}

/// Verify STL bytes without changing or repairing them.
pub fn verify_bytes(bytes: &[u8]) -> Result<StlIntegrityReport, StlIntegrityError> {
    let parsed = parse(bytes)?;
    verify_mesh(parsed)
}

#[derive(Debug)]
struct ParsedStl {
    format: StlFormat,
    triangles: Vec<RawTriangle>,
}

#[derive(Debug, Clone, Copy)]
struct RawTriangle {
    vertices: [[f64; 3]; 3],
}

fn parse(bytes: &[u8]) -> Result<ParsedStl, StlIntegrityError> {
    if bytes.is_empty() {
        return Err(StlIntegrityError::new(
            IntegrityReason::Empty,
            "STL is empty",
        ));
    }

    if looks_like_ascii(bytes) {
        let ascii = parse_ascii(bytes);
        if ascii.is_ok() || !binary_size_matches(bytes) {
            ascii
        } else {
            parse_binary(bytes)
        }
    } else {
        parse_binary(bytes)
    }
}

fn looks_like_ascii(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    !bytes.contains(&0)
        && text
            .lines()
            .next()
            .is_some_and(|line| line.split_whitespace().next() == Some("solid"))
}

fn parse_ascii(bytes: &[u8]) -> Result<ParsedStl, StlIntegrityError> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        StlIntegrityError::new(
            IntegrityReason::Format,
            format!("ASCII STL is not UTF-8: {error}"),
        )
    })?;
    if !bytes.is_ascii() {
        return Err(StlIntegrityError::new(
            IntegrityReason::Format,
            "ASCII STL contains a non-ASCII byte",
        ));
    }

    let mut lines = text.lines().enumerate();
    let Some((header_line, header)) = lines.next() else {
        return Err(StlIntegrityError::new(
            IntegrityReason::Empty,
            "STL has no header",
        ));
    };
    if header.split_whitespace().next() != Some("solid") {
        return Err(StlIntegrityError::at_line(
            IntegrityReason::Format,
            header_line + 1,
            "ASCII STL must begin with solid",
        ));
    }

    let mut triangles = Vec::new();
    loop {
        let Some((line_number, line)) = lines.next() else {
            return Err(StlIntegrityError::new(
                IntegrityReason::Truncated,
                "ASCII STL is missing endsolid",
            ));
        };
        let tokens: Vec<_> = line.split_whitespace().collect();
        match tokens.first().copied() {
            Some("endsolid") => {
                if triangles.is_empty() {
                    return Err(StlIntegrityError::new(
                        IntegrityReason::Empty,
                        "ASCII STL has no facets",
                    ));
                }
                for (trailing_line, trailing) in lines {
                    if !trailing.trim().is_empty() {
                        return Err(StlIntegrityError::at_line(
                            IntegrityReason::Format,
                            trailing_line + 1,
                            "data follows endsolid",
                        ));
                    }
                }
                return Ok(ParsedStl {
                    format: StlFormat::Ascii,
                    triangles,
                });
            }
            Some("facet") => {
                if tokens.len() != 5 || tokens[1] != "normal" {
                    return Err(StlIntegrityError::at_line(
                        IntegrityReason::Format,
                        line_number + 1,
                        "facet must be followed by normal and three values",
                    ));
                }
                let normal = [
                    parse_ascii_number(tokens[2], line_number + 1)?,
                    parse_ascii_number(tokens[3], line_number + 1)?,
                    parse_ascii_number(tokens[4], line_number + 1)?,
                ];
                expect_ascii_line(&mut lines, "outer loop")?;
                let mut vertices = [[0.0; 3]; 3];
                for vertex in &mut vertices {
                    let (vertex_line, vertex_tokens) = next_ascii_tokens(&mut lines)?;
                    if vertex_tokens.len() != 4 || vertex_tokens[0] != "vertex" {
                        return Err(StlIntegrityError::at_line(
                            IntegrityReason::Format,
                            vertex_line,
                            "outer loop must contain exactly three vertex records",
                        ));
                    }
                    *vertex = [
                        parse_ascii_number(vertex_tokens[1], vertex_line)?,
                        parse_ascii_number(vertex_tokens[2], vertex_line)?,
                        parse_ascii_number(vertex_tokens[3], vertex_line)?,
                    ];
                }
                expect_ascii_line(&mut lines, "endloop")?;
                expect_ascii_line(&mut lines, "endfacet")?;
                let _ = normal;
                triangles.push(RawTriangle { vertices });
            }
            Some(_) | None => {
                return Err(StlIntegrityError::at_line(
                    IntegrityReason::Format,
                    line_number + 1,
                    "expected facet or endsolid",
                ));
            }
        }
    }
}

fn next_ascii_tokens<'a, I>(lines: &mut I) -> Result<(usize, Vec<&'a str>), StlIntegrityError>
where
    I: Iterator<Item = (usize, &'a str)>,
{
    let Some((line, text)) = lines.next() else {
        return Err(StlIntegrityError::new(
            IntegrityReason::Truncated,
            "ASCII STL facet is truncated",
        ));
    };
    Ok((line + 1, text.split_whitespace().collect()))
}

fn expect_ascii_line<'a, I>(lines: &mut I, expected: &str) -> Result<(), StlIntegrityError>
where
    I: Iterator<Item = (usize, &'a str)>,
{
    let (line, tokens) = next_ascii_tokens(lines)?;
    if tokens != expected.split_whitespace().collect::<Vec<_>>() {
        return Err(StlIntegrityError::at_line(
            IntegrityReason::Format,
            line,
            format!("expected {expected}"),
        ));
    }
    Ok(())
}

fn parse_ascii_number(token: &str, line: usize) -> Result<f64, StlIntegrityError> {
    let value = token.parse::<f64>().map_err(|error| {
        StlIntegrityError::at_line(
            IntegrityReason::Format,
            line,
            format!("invalid numeric token {token:?}: {error}"),
        )
    })?;
    if !value.is_finite() {
        return Err(StlIntegrityError::at_line(
            IntegrityReason::NonFinite,
            line,
            "STL numeric values must be finite",
        ));
    }
    Ok(value)
}

fn parse_binary(bytes: &[u8]) -> Result<ParsedStl, StlIntegrityError> {
    if bytes.len() < 84 {
        return Err(StlIntegrityError::new(
            IntegrityReason::Truncated,
            format!("binary STL header is truncated at {} bytes", bytes.len()),
        ));
    }
    let count = u32::from_le_bytes(bytes[80..84].try_into().expect("count is four bytes"));
    let expected = 84usize
        .checked_add((count as usize).checked_mul(50).ok_or_else(|| {
            StlIntegrityError::new(
                IntegrityReason::SizeMismatch,
                "binary STL triangle count overflows the declared size",
            )
        })?)
        .ok_or_else(|| {
            StlIntegrityError::new(
                IntegrityReason::SizeMismatch,
                "binary STL declared size overflows the host address space",
            )
        })?;
    if bytes.len() < expected {
        return Err(StlIntegrityError::new(
            IntegrityReason::Truncated,
            format!(
                "binary STL declares {count} triangles but ends at {} bytes",
                bytes.len()
            ),
        ));
    }
    if bytes.len() > expected {
        return Err(StlIntegrityError::new(
            IntegrityReason::SizeMismatch,
            format!(
                "binary STL declares {count} triangles for {expected} bytes but contains {}",
                bytes.len()
            ),
        ));
    }
    if count == 0 {
        return Err(StlIntegrityError::new(
            IntegrityReason::Empty,
            "binary STL declares zero triangles",
        ));
    }

    let mut triangles = Vec::with_capacity(count as usize);
    for triangle in 0..count as usize {
        let start = 84 + triangle * 50;
        let mut values = [0.0; 12];
        for (index, value) in values.iter_mut().enumerate() {
            let offset = start + index * 4;
            let bits = u32::from_le_bytes(
                bytes[offset..offset + 4]
                    .try_into()
                    .expect("binary STL float is four bytes"),
            );
            *value = f32::from_bits(bits) as f64;
            if !value.is_finite() {
                return Err(StlIntegrityError::at_triangle(
                    IntegrityReason::NonFinite,
                    triangle,
                    "binary STL normal and vertices must be finite",
                ));
            }
        }
        triangles.push(RawTriangle {
            vertices: [
                [values[3], values[4], values[5]],
                [values[6], values[7], values[8]],
                [values[9], values[10], values[11]],
            ],
        });
    }
    Ok(ParsedStl {
        format: StlFormat::Binary,
        triangles,
    })
}

fn binary_size_matches(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        return false;
    }
    let count = u32::from_le_bytes(bytes[80..84].try_into().expect("count is four bytes"));
    84usize.saturating_add((count as usize).saturating_mul(50)) == bytes.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VertexKey([u64; 3]);

fn vertex_key(vertex: [f64; 3]) -> VertexKey {
    VertexKey(vertex.map(|value| if value == 0.0 { 0 } else { value.to_bits() }))
}

#[derive(Debug, Clone, Copy)]
struct Triangle {
    vertices: [[f64; 3]; 3],
    indices: [usize; 3],
}

#[derive(Debug, Clone, Copy)]
struct EdgeUse {
    triangle: usize,
    from: usize,
    to: usize,
}

fn verify_mesh(parsed: ParsedStl) -> Result<StlIntegrityReport, StlIntegrityError> {
    let mut vertices = Vec::<[f64; 3]>::new();
    let mut vertex_ids = BTreeMap::<VertexKey, usize>::new();
    let mut triangles = Vec::with_capacity(parsed.triangles.len());
    let mut facets = BTreeSet::<[usize; 3]>::new();

    for (triangle_index, raw) in parsed.triangles.iter().enumerate() {
        let mut indices = [0; 3];
        for (index, vertex) in raw.vertices.into_iter().enumerate() {
            let key = vertex_key(vertex);
            indices[index] = *vertex_ids.entry(key).or_insert_with(|| {
                let id = vertices.len();
                vertices.push(vertex);
                id
            });
        }
        if indices[0] == indices[1] || indices[1] == indices[2] || indices[2] == indices[0] {
            return Err(StlIntegrityError::at_triangle(
                IntegrityReason::Degenerate,
                triangle_index,
                "triangle has repeated vertices",
            ));
        }
        let cross = cross(
            sub(raw.vertices[1], raw.vertices[0]),
            sub(raw.vertices[2], raw.vertices[0]),
        );
        let area_squared = dot(cross, cross);
        if !area_squared.is_finite() {
            return Err(StlIntegrityError::at_triangle(
                IntegrityReason::NonFinite,
                triangle_index,
                "triangle area is not finite",
            ));
        }
        if area_squared == 0.0 {
            return Err(StlIntegrityError::at_triangle(
                IntegrityReason::Degenerate,
                triangle_index,
                "triangle area is zero or below the strict threshold",
            ));
        }
        let mut facet = indices;
        facet.sort_unstable();
        if !facets.insert(facet) {
            return Err(StlIntegrityError::at_triangle(
                IntegrityReason::DuplicateFacet,
                triangle_index,
                "facet repeats an earlier triangle",
            ));
        }
        triangles.push(Triangle {
            vertices: raw.vertices,
            indices,
        });
    }

    let mut edges = BTreeMap::<(usize, usize), Vec<EdgeUse>>::new();
    let mut vertex_incidence = BTreeMap::<usize, Vec<usize>>::new();
    for (triangle, value) in triangles.iter().enumerate() {
        for vertex in value.indices {
            vertex_incidence.entry(vertex).or_default().push(triangle);
        }
        for (from, to) in [
            (value.indices[0], value.indices[1]),
            (value.indices[1], value.indices[2]),
            (value.indices[2], value.indices[0]),
        ] {
            let key = if from < to { (from, to) } else { (to, from) };
            edges
                .entry(key)
                .or_default()
                .push(EdgeUse { triangle, from, to });
        }
    }

    let mut triangle_neighbors = vec![Vec::new(); triangles.len()];
    for (edge, uses) in &edges {
        match uses.as_slice() {
            [first, second] => {
                if first.from == second.from && first.to == second.to {
                    return Err(StlIntegrityError::at_triangle(
                        IntegrityReason::InconsistentOrientation,
                        second.triangle,
                        format!("shared edge {edge:?} has the same winding on both facets"),
                    ));
                }
                triangle_neighbors[first.triangle].push(second.triangle);
                triangle_neighbors[second.triangle].push(first.triangle);
            }
            [] => unreachable!("an edge was just inserted"),
            [_] => {
                return Err(StlIntegrityError::new(
                    IntegrityReason::OpenBoundary,
                    format!("edge {edge:?} has only one incident facet"),
                ));
            }
            _ => {
                return Err(StlIntegrityError::new(
                    IntegrityReason::NonManifold,
                    format!("edge {edge:?} has more than two incident facets"),
                ));
            }
        }
    }

    check_vertex_links(&edges, &vertex_incidence)?;
    let shells = connected_shells(&triangle_neighbors, &triangles);
    let shell_signed_volumes: Vec<_> = shells
        .iter()
        .map(|shell| {
            shell
                .iter()
                .map(|&triangle| signed_triangle_volume(triangles[triangle].vertices))
                .sum::<f64>()
        })
        .collect();
    let positive_shells: Vec<_> = shell_signed_volumes
        .iter()
        .enumerate()
        .filter_map(|(index, volume)| (*volume > 0.0).then_some(index))
        .collect();
    if positive_shells.is_empty() {
        return Err(StlIntegrityError::new(
            IntegrityReason::NonPositiveVolume,
            "mesh has no positively oriented material exterior",
        ));
    }
    if positive_shells.len() != 1 {
        return Err(StlIntegrityError::new(
            IntegrityReason::MaterialBodyCount,
            format!(
                "mesh has {} positively oriented material shells",
                positive_shells.len()
            ),
        ));
    }
    let exterior = positive_shells[0];
    let mut cavity_shell_count = 0;
    for (shell, volume) in shell_signed_volumes.iter().enumerate() {
        if shell == exterior {
            continue;
        }
        if *volume >= 0.0 {
            return Err(StlIntegrityError::new(
                IntegrityReason::NonPositiveVolume,
                format!("shell {shell} has no strictly negative cavity orientation"),
            ));
        }
        if !shell_vertices_strictly_inside(&shells[shell], &shells[exterior], &triangles, &vertices)
            || shells_intersect(&shells[shell], &shells[exterior], &triangles)
        {
            return Err(StlIntegrityError::new(
                IntegrityReason::CavityContainment,
                format!("cavity shell {shell} is not strictly contained by the exterior"),
            ));
        }
        cavity_shell_count += 1;
    }
    for (left_index, left) in shell_signed_volumes.iter().enumerate() {
        if *left >= 0.0 {
            continue;
        }
        for (right_index, right) in shell_signed_volumes.iter().enumerate().skip(left_index + 1) {
            if *right < 0.0
                && (shells_intersect(&shells[left_index], &shells[right_index], &triangles)
                    || shell_vertices_strictly_inside(
                        &shells[left_index],
                        &shells[right_index],
                        &triangles,
                        &vertices,
                    )
                    || shell_vertices_strictly_inside(
                        &shells[right_index],
                        &shells[left_index],
                        &triangles,
                        &vertices,
                    ))
            {
                return Err(StlIntegrityError::new(
                    IntegrityReason::CavityContainment,
                    format!("cavity shells {left_index} and {right_index} overlap or nest"),
                ));
            }
        }
    }
    let signed_volume = shell_signed_volumes.iter().sum::<f64>();
    if !signed_volume.is_finite() || signed_volume <= 0.0 {
        return Err(StlIntegrityError::new(
            IntegrityReason::NonPositiveVolume,
            format!("material volume is not positive: {signed_volume}"),
        ));
    }
    Ok(StlIntegrityReport {
        format: parsed.format,
        triangle_count: triangles.len(),
        unique_vertex_count: vertices.len(),
        shell_count: shells.len(),
        cavity_shell_count,
        shell_signed_volumes,
        signed_volume,
        material_volume: signed_volume,
    })
}

fn check_vertex_links(
    edges: &BTreeMap<(usize, usize), Vec<EdgeUse>>,
    incidence: &BTreeMap<usize, Vec<usize>>,
) -> Result<(), StlIntegrityError> {
    let mut links = BTreeMap::<usize, BTreeMap<usize, BTreeSet<usize>>>::new();
    for (&edge, uses) in edges {
        let [first, second] = uses.as_slice() else {
            continue;
        };
        for vertex in [edge.0, edge.1] {
            let neighbors = links.entry(vertex).or_default();
            neighbors
                .entry(first.triangle)
                .or_default()
                .insert(second.triangle);
            neighbors
                .entry(second.triangle)
                .or_default()
                .insert(first.triangle);
        }
    }
    for (&vertex, incident) in incidence {
        let Some(link) = links.get(&vertex) else {
            return Err(StlIntegrityError::new(
                IntegrityReason::NonManifold,
                format!("vertex {vertex} has no link neighborhood"),
            ));
        };
        if link.len() != incident.len() || link.values().any(|neighbors| neighbors.len() != 2) {
            return Err(StlIntegrityError::new(
                IntegrityReason::NonManifold,
                format!("vertex {vertex} does not have a degree-two link"),
            ));
        }
        let start = incident[0];
        let mut seen = BTreeSet::new();
        let mut pending = vec![start];
        while let Some(triangle) = pending.pop() {
            if !seen.insert(triangle) {
                continue;
            }
            pending.extend(link[&triangle].iter().copied());
        }
        if seen.len() != incident.len() {
            return Err(StlIntegrityError::new(
                IntegrityReason::NonManifold,
                format!("vertex {vertex} has a disconnected link neighborhood"),
            ));
        }
    }
    Ok(())
}

fn connected_shells(neighbors: &[Vec<usize>], triangles: &[Triangle]) -> Vec<Vec<usize>> {
    let mut shells = Vec::new();
    let mut visited = vec![false; triangles.len()];
    for start in 0..triangles.len() {
        if visited[start] {
            continue;
        }
        let mut shell = Vec::new();
        let mut pending = VecDeque::from([start]);
        visited[start] = true;
        while let Some(triangle) = pending.pop_front() {
            shell.push(triangle);
            for &neighbor in &neighbors[triangle] {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    pending.push_back(neighbor);
                }
            }
        }
        shells.push(shell);
    }
    shells
}

fn shell_vertices_strictly_inside(
    inner: &[usize],
    outer: &[usize],
    triangles: &[Triangle],
    coordinates: &[[f64; 3]],
) -> bool {
    let mut vertex_ids = BTreeSet::new();
    for &triangle in inner {
        vertex_ids.extend(triangles[triangle].indices);
    }
    let outer_triangles: Vec<_> = outer.iter().map(|&index| triangles[index]).collect();
    vertex_ids
        .into_iter()
        .all(|vertex| point_inside_shell(coordinates[vertex], &outer_triangles))
}

fn shells_intersect(left: &[usize], right: &[usize], triangles: &[Triangle]) -> bool {
    left.iter().any(|&left_triangle| {
        right.iter().any(|&right_triangle| {
            triangles_intersect(
                triangles[left_triangle].vertices,
                triangles[right_triangle].vertices,
            )
        })
    })
}

fn triangles_intersect(left: [[f64; 3]; 3], right: [[f64; 3]; 3]) -> bool {
    let left_edges = [(left[0], left[1]), (left[1], left[2]), (left[2], left[0])];
    let right_edges = [
        (right[0], right[1]),
        (right[1], right[2]),
        (right[2], right[0]),
    ];
    left_edges
        .into_iter()
        .any(|(start, end)| segment_intersects_triangle(start, end, right))
        || right_edges
            .into_iter()
            .any(|(start, end)| segment_intersects_triangle(start, end, left))
}

fn segment_intersects_triangle(start: [f64; 3], end: [f64; 3], triangle: [[f64; 3]; 3]) -> bool {
    let [a, b, c] = triangle;
    let direction = sub(end, start);
    let edge1 = sub(b, a);
    let edge2 = sub(c, a);
    let pvec = cross(direction, edge2);
    let determinant = dot(edge1, pvec);
    if determinant.abs() <= GEOMETRIC_EPSILON {
        let normal = cross(edge1, edge2);
        if dot(sub(start, a), normal).abs() > GEOMETRIC_EPSILON
            || dot(sub(end, a), normal).abs() > GEOMETRIC_EPSILON
        {
            return false;
        }
        let axis = dominant_axis(normal);
        let segment_start = project(start, axis);
        let segment_end = project(end, axis);
        let projected = triangle.map(|vertex| project(vertex, axis));
        return point_in_triangle_2d(segment_start, projected)
            || point_in_triangle_2d(segment_end, projected)
            || [
                (projected[0], projected[1]),
                (projected[1], projected[2]),
                (projected[2], projected[0]),
            ]
            .into_iter()
            .any(|(left, right)| segments_intersect_2d(segment_start, segment_end, left, right));
    }
    let inverse = 1.0 / determinant;
    let tvec = sub(start, a);
    let u = dot(tvec, pvec) * inverse;
    let qvec = cross(tvec, edge1);
    let v = dot(direction, qvec) * inverse;
    let t = dot(edge2, qvec) * inverse;
    (-GEOMETRIC_EPSILON..=1.0 + GEOMETRIC_EPSILON).contains(&t)
        && u >= -GEOMETRIC_EPSILON
        && v >= -GEOMETRIC_EPSILON
        && u + v <= 1.0 + GEOMETRIC_EPSILON
}

fn dominant_axis(normal: [f64; 3]) -> usize {
    let magnitudes = normal.map(f64::abs);
    if magnitudes[1] > magnitudes[0] && magnitudes[1] >= magnitudes[2] {
        1
    } else if magnitudes[2] > magnitudes[0] && magnitudes[2] > magnitudes[1] {
        2
    } else {
        0
    }
}

fn project(point: [f64; 3], axis: usize) -> [f64; 2] {
    match axis {
        0 => [point[1], point[2]],
        1 => [point[0], point[2]],
        _ => [point[0], point[1]],
    }
}

fn point_in_triangle_2d(point: [f64; 2], triangle: [[f64; 2]; 3]) -> bool {
    let [a, b, c] = triangle;
    let first = orient_2d(a, b, point);
    let second = orient_2d(b, c, point);
    let third = orient_2d(c, a, point);
    (first >= -GEOMETRIC_EPSILON && second >= -GEOMETRIC_EPSILON && third >= -GEOMETRIC_EPSILON)
        || (first <= GEOMETRIC_EPSILON && second <= GEOMETRIC_EPSILON && third <= GEOMETRIC_EPSILON)
}

fn segments_intersect_2d(
    left_start: [f64; 2],
    left_end: [f64; 2],
    right_start: [f64; 2],
    right_end: [f64; 2],
) -> bool {
    let first = orient_2d(left_start, left_end, right_start);
    let second = orient_2d(left_start, left_end, right_end);
    let third = orient_2d(right_start, right_end, left_start);
    let fourth = orient_2d(right_start, right_end, left_end);
    (first.abs() <= GEOMETRIC_EPSILON && point_on_segment_2d(right_start, left_start, left_end))
        || (second.abs() <= GEOMETRIC_EPSILON
            && point_on_segment_2d(right_end, left_start, left_end))
        || (third.abs() <= GEOMETRIC_EPSILON
            && point_on_segment_2d(left_start, right_start, right_end))
        || (fourth.abs() <= GEOMETRIC_EPSILON
            && point_on_segment_2d(left_end, right_start, right_end))
        || ((first > 0.0) != (second > 0.0) && (third > 0.0) != (fourth > 0.0))
}

fn point_on_segment_2d(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> bool {
    point[0] >= start[0].min(end[0]) - GEOMETRIC_EPSILON
        && point[0] <= start[0].max(end[0]) + GEOMETRIC_EPSILON
        && point[1] >= start[1].min(end[1]) - GEOMETRIC_EPSILON
        && point[1] <= start[1].max(end[1]) + GEOMETRIC_EPSILON
}

fn orient_2d(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn point_inside_shell(point: [f64; 3], triangles: &[Triangle]) -> bool {
    if triangles
        .iter()
        .any(|triangle| point_on_triangle(point, triangle.vertices))
    {
        return false;
    }
    let directions = [
        [1.0, 0.371_390_676_354_103_7, 0.173_205_080_756_887_7],
        [0.219_871, 1.0, 0.414_213],
        [0.137_503, 0.281_731, 1.0],
    ];
    directions
        .into_iter()
        .find_map(|direction| ray_parity(point, direction, triangles))
        .unwrap_or(false)
}

fn ray_parity(point: [f64; 3], direction: [f64; 3], triangles: &[Triangle]) -> Option<bool> {
    let mut intersections = 0;
    for triangle in triangles {
        let [a, b, c] = triangle.vertices;
        let edge1 = sub(b, a);
        let edge2 = sub(c, a);
        let pvec = cross(direction, edge2);
        let determinant = dot(edge1, pvec);
        if determinant.abs() <= GEOMETRIC_EPSILON {
            continue;
        }
        let inverse = 1.0 / determinant;
        let tvec = sub(point, a);
        let u = dot(tvec, pvec) * inverse;
        let qvec = cross(tvec, edge1);
        let v = dot(direction, qvec) * inverse;
        let t = dot(edge2, qvec) * inverse;
        if t <= GEOMETRIC_EPSILON {
            continue;
        }
        if u < -GEOMETRIC_EPSILON || v < -GEOMETRIC_EPSILON || u + v > 1.0 + GEOMETRIC_EPSILON {
            continue;
        }
        if u <= GEOMETRIC_EPSILON || v <= GEOMETRIC_EPSILON || (1.0 - u - v) <= GEOMETRIC_EPSILON {
            return None;
        }
        intersections += 1;
    }
    Some(intersections % 2 == 1)
}

fn point_on_triangle(point: [f64; 3], vertices: [[f64; 3]; 3]) -> bool {
    let [a, b, c] = vertices;
    let normal = cross(sub(b, a), sub(c, a));
    let normal_length = dot(normal, normal).sqrt();
    if normal_length == 0.0 {
        return false;
    }
    if dot(sub(point, a), normal).abs() > GEOMETRIC_EPSILON * normal_length {
        return false;
    }
    let v0 = sub(b, a);
    let v1 = sub(c, a);
    let v2 = sub(point, a);
    let dot00 = dot(v0, v0);
    let dot01 = dot(v0, v1);
    let dot02 = dot(v0, v2);
    let dot11 = dot(v1, v1);
    let dot12 = dot(v1, v2);
    let denominator = dot00 * dot11 - dot01 * dot01;
    if denominator.abs() <= f64::EPSILON {
        return false;
    }
    let inverse = 1.0 / denominator;
    let u = (dot11 * dot02 - dot01 * dot12) * inverse;
    let v = (dot00 * dot12 - dot01 * dot02) * inverse;
    u >= -GEOMETRIC_EPSILON && v >= -GEOMETRIC_EPSILON && u + v <= 1.0 + GEOMETRIC_EPSILON
}

fn signed_triangle_volume(vertices: [[f64; 3]; 3]) -> f64 {
    dot(vertices[0], cross(vertices[1], vertices[2])) / 6.0
}

fn sub(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}
