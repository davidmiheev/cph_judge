//! Lists of edges: making trees and simple graphs, and checking that a list is one.
//!
//! Used by the generator (to make a list), by the sample checker (to warn when a sample is not what its
//! constraint says) and, through that checker, by the reducer (to keep a simplified list valid).
//! Vertices are numbered from `lo`; the functions that make edges work with `0..n` and the caller adds `lo`.

use std::collections::HashSet;

use crate::generate::Rng;
use crate::schema::{EdgeKind, EdgeSpec};

/// More vertices than this is a mistake in the constraints, not a test.
const MAX_VERTICES: usize = 1_000_000;

/// A random labelled tree on `n` vertices, as `n - 1` edges. The shape varies from test to test (random,
/// a path, a star, a caterpillar-ish mix), because paths and stars are where solutions break.
pub fn random_tree(rng: &mut Rng, n: usize) -> Vec<(usize, usize)> {
    let mut labels: Vec<usize> = (0..n).collect();
    rng.shuffle(&mut labels);
    let shape = rng.range(0, 3);
    (1..n)
        .map(|i| {
            let parent = match shape {
                0 => rng.range(0, i as i64 - 1) as usize,                         // any earlier vertex
                1 => i - 1,                                                       // a path
                2 => 0,                                                           // a star
                _ => rng.range(i.saturating_sub(3) as i64, i as i64 - 1) as usize, // one of the last few
            };
            (labels[parent], labels[i])
        })
        .collect()
}

/// The most edges a simple graph on `n` vertices can have.
pub fn max_edges(n: usize, directed: bool) -> usize {
    let pairs = n.saturating_mul(n.saturating_sub(1));
    if directed { pairs } else { pairs / 2 }
}

/// `m` edges of a tree or simple graph on the vertices `0..n`, in random order and orientation.
pub fn make_edges(rng: &mut Rng, n: usize, m: usize, spec: &EdgeSpec) -> Result<Vec<(usize, usize)>, String> {
    if n == 0 || n > MAX_VERTICES {
        return Err(format!("a list of edges cannot be made on {n} vertices"));
    }
    let mut edges = match spec.kind {
        EdgeKind::Tree => {
            if m + 1 != n {
                return Err(format!("a tree on {n} vertices has {} edges, but the number of edges came out as {m}", n - 1));
            }
            random_tree(rng, n)
        }
        EdgeKind::Graph => {
            let max = max_edges(n, spec.directed);
            if m > max {
                return Err(format!("{m} edges do not fit a simple graph on {n} vertices (at most {max})"));
            }
            if spec.connected && m + 1 < n {
                return Err(format!("{m} edges cannot connect {n} vertices (at least {} are needed)", n - 1));
            }
            let key = |a: usize, b: usize| if spec.directed { (a, b) } else { (a.min(b), a.max(b)) };
            let mut chosen = if spec.connected { random_tree(rng, n) } else { Vec::new() };
            let mut taken: HashSet<(usize, usize)> = chosen.iter().map(|&(a, b)| key(a, b)).collect();
            let need = m - chosen.len();
            if need * 2 >= max - chosen.len() && n <= 4000 {
                // Most pairs are wanted: list the unused ones, shuffle, and take what is needed.
                let mut free = Vec::new();
                for a in 0..n {
                    for b in (if spec.directed { 0 } else { a + 1 })..n {
                        if a != b && !taken.contains(&key(a, b)) {
                            free.push((a, b));
                        }
                    }
                }
                rng.shuffle(&mut free);
                chosen.extend(free.into_iter().take(need));
            } else {
                // Few are wanted among many: draw until there are enough different ones.
                while chosen.len() < m {
                    let (a, b) = (rng.range(0, n as i64 - 1) as usize, rng.range(0, n as i64 - 1) as usize);
                    if a != b && taken.insert(key(a, b)) {
                        chosen.push((a, b));
                    }
                }
            }
            chosen
        }
    };

    for edge in &mut edges {
        let flip = if spec.directed { false } else if spec.ordered { edge.0 > edge.1 } else { rng.range(0, 1) == 1 };
        if flip {
            *edge = (edge.1, edge.0);
        }
    }
    rng.shuffle(&mut edges);
    Ok(edges)
}

/// Whether `rows` (vertices numbered from `lo`) connect all `n` vertices.
fn connects(n: usize, rows: &[(i64, i64)], lo: i64) -> bool {
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut components = n;
    for &(u, v) in rows {
        let (a, b) = ((u - lo) as usize, (v - lo) as usize);
        if a < n && b < n {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent[ra] = rb;
                components -= 1;
            }
        }
    }
    components <= 1
}

/// What is wrong with a list of edges on the vertices `lo..lo + n`, if anything.
pub fn check_edges(spec: &EdgeSpec, rows: &[(i64, i64)], lo: i64, n: usize) -> Option<String> {
    let hi = lo + n as i64 - 1;
    if let Some(&bad) = rows.iter().flat_map(|(u, v)| [u, v]).find(|&&x| x < lo || x > hi) {
        return Some(format!("the vertex {bad} is outside {lo}..{hi}"));
    }
    if spec.kind == EdgeKind::Tree && rows.len() + 1 != n {
        return Some(format!("a tree on {n} vertices has {} edges, not {}", n.saturating_sub(1), rows.len()));
    }
    let mut seen = HashSet::new();
    for &(u, v) in rows {
        if u == v {
            return Some(format!("the edge {u} {v} is a loop"));
        }
        if spec.ordered && u > v {
            return Some(format!("the edge {u} {v} is not ordered: the smaller vertex should come first"));
        }
        let key = if spec.directed { (u, v) } else { (u.min(v), u.max(v)) };
        if !seen.insert(key) {
            return Some(format!("the edge {u} {v} is repeated{}", if spec.directed { "" } else { " (an edge and its reverse are the same)" }));
        }
    }
    if spec.connected && !connects(n, rows, lo) {
        return Some(format!("the edges do not connect all {n} vertices"));
    }
    None
}

/// The constraint on the number of edges that goes with this kind of list, over `vertices` (the name
/// of the number of vertices): `n-1..n-1` for a tree, `0..n*(n-1)/2` for a graph, and so on.
pub fn count_range(spec: &EdgeSpec, vertices: &str) -> String {
    let v = vertices;
    match spec.kind {
        EdgeKind::Tree => format!("{v}-1..{v}-1"),
        EdgeKind::Graph => {
            let low = if spec.connected { format!("{v}-1") } else { "0".to_string() };
            let high = if spec.directed { format!("{v}*({v}-1)") } else { format!("{v}*({v}-1)/2") };
            format!("{low}..{high}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: EdgeKind, directed: bool, connected: bool, ordered: bool) -> EdgeSpec {
        EdgeSpec { kind, directed, connected: connected || kind == EdgeKind::Tree, ordered }
    }

    fn one_based(edges: &[(usize, usize)]) -> Vec<(i64, i64)> {
        edges.iter().map(|&(a, b)| (a as i64 + 1, b as i64 + 1)).collect()
    }

    fn degrees(n: usize, edges: &[(usize, usize)]) -> Vec<usize> {
        let mut d = vec![0; n];
        for &(a, b) in edges {
            d[a] += 1;
            d[b] += 1;
        }
        d
    }

    // ---- making ------------------------------------------------------------------------------

    #[test]
    fn trees_are_trees_of_every_size() {
        let tree = spec(EdgeKind::Tree, false, true, false);
        for seed in 0..200 {
            for n in 1..=12 {
                let edges = make_edges(&mut Rng::new(seed), n, n - 1, &tree).unwrap();
                assert_eq!(check_edges(&tree, &one_based(&edges), 1, n), None, "seed {seed} n {n}: {edges:?}");
            }
        }
    }

    #[test]
    fn trees_come_in_different_shapes_including_paths_and_stars() {
        let tree = spec(EdgeKind::Tree, false, true, false);
        let max_degrees: HashSet<usize> =
            (0..300).map(|seed| *degrees(8, &make_edges(&mut Rng::new(seed), 8, 7, &tree).unwrap()).iter().max().unwrap()).collect();
        assert!(max_degrees.contains(&2), "never a path: {max_degrees:?}");
        assert!(max_degrees.contains(&7), "never a star: {max_degrees:?}");
        assert!(max_degrees.len() >= 4, "{max_degrees:?}");
    }

    #[test]
    fn simple_graphs_have_no_loops_or_repeated_edges() {
        for (directed, connected) in [(false, false), (false, true), (true, false), (true, true)] {
            let graph = spec(EdgeKind::Graph, directed, connected, false);
            for seed in 0..60 {
                for n in 1..=7 {
                    let max = max_edges(n, directed);
                    for m in (if connected { n - 1 } else { 0 })..=max {
                        let edges = make_edges(&mut Rng::new(seed), n, m, &graph).unwrap();
                        assert_eq!(edges.len(), m);
                        assert_eq!(check_edges(&graph, &one_based(&edges), 1, n), None, "directed {directed} connected {connected} n {n} m {m}: {edges:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn ordered_lists_always_put_the_smaller_vertex_first() {
        for kind in [spec(EdgeKind::Tree, false, true, true), spec(EdgeKind::Graph, false, false, true)] {
            for seed in 0..100 {
                let edges = make_edges(&mut Rng::new(seed), 6, 5, &kind).unwrap();
                assert!(edges.iter().all(|&(a, b)| a < b), "{edges:?}");
            }
        }
    }

    #[test]
    fn unordered_lists_use_both_orientations() {
        let graph = spec(EdgeKind::Graph, false, false, false);
        let edges: Vec<(usize, usize)> = (0..20).flat_map(|seed| make_edges(&mut Rng::new(seed), 6, 8, &graph).unwrap()).collect();
        assert!(edges.iter().any(|&(a, b)| a < b) && edges.iter().any(|&(a, b)| a > b));
    }

    #[test]
    fn a_nearly_complete_graph_is_made_without_endless_drawing() {
        let graph = spec(EdgeKind::Graph, false, false, false);
        let edges = make_edges(&mut Rng::new(1), 30, max_edges(30, false) - 1, &graph).unwrap();
        assert_eq!(check_edges(&graph, &one_based(&edges), 1, 30), None);
    }

    #[test]
    fn impossible_requests_are_refused_with_the_numbers() {
        let mut rng = Rng::new(1);
        let error = |spec: EdgeSpec, n, m| make_edges(&mut Rng::new(1), n, m, &spec).unwrap_err();
        assert!(error(spec(EdgeKind::Tree, false, true, false), 5, 3).contains("a tree on 5 vertices has 4 edges"));
        assert!(error(spec(EdgeKind::Graph, false, false, false), 4, 7).contains("at most 6"));
        assert!(error(spec(EdgeKind::Graph, true, false, false), 4, 13).contains("at most 12"));
        assert!(error(spec(EdgeKind::Graph, false, true, false), 6, 4).contains("at least 5"));
        assert!(error(spec(EdgeKind::Graph, false, false, false), 0, 0).contains("0 vertices"));
        let _ = &mut rng;
    }

    #[test]
    fn a_single_vertex_has_no_edges() {
        let tree = spec(EdgeKind::Tree, false, true, false);
        assert_eq!(make_edges(&mut Rng::new(1), 1, 0, &tree).unwrap(), vec![]);
    }

    // ---- checking ------------------------------------------------------------------------------

    #[test]
    fn checking_names_what_is_wrong() {
        let tree = spec(EdgeKind::Tree, false, true, false);
        let graph = spec(EdgeKind::Graph, false, false, false);
        let digraph = spec(EdgeKind::Graph, true, false, false);
        let ordered = spec(EdgeKind::Graph, false, false, true);
        let connected = spec(EdgeKind::Graph, false, true, false);
        let check = |spec: &EdgeSpec, rows: &[(i64, i64)], n| check_edges(spec, rows, 1, n).unwrap_or_default();

        assert!(check(&tree, &[(1, 2)], 4).contains("a tree on 4 vertices has 3 edges, not 1"));
        assert!(check(&tree, &[(1, 2), (2, 1), (3, 4)], 4).contains("repeated"));
        assert!(check(&tree, &[(1, 2), (3, 4), (3, 4)], 4).contains("repeated"));
        assert!(check(&tree, &[(1, 2), (1, 2), (3, 4)], 4).contains("repeated"));
        assert!(check(&tree, &[(1, 2), (3, 4), (2, 1)], 4).contains("repeated"));
        assert!(check(&tree, &[(1, 2), (3, 4), (1, 1)], 4).contains("loop"));
        assert!(check(&tree, &[(1, 2), (1, 3), (2, 3)], 4).contains("do not connect"), "a triangle and a lone vertex");
        assert!(check(&graph, &[(2, 2)], 3).contains("loop"));
        assert!(check(&graph, &[(1, 2), (2, 1)], 3).contains("an edge and its reverse are the same"));
        assert!(check(&digraph, &[(1, 2), (1, 2)], 3).contains("repeated"));
        assert!(check(&ordered, &[(3, 1)], 3).contains("not ordered"));
        assert!(check(&connected, &[(1, 2)], 3).contains("do not connect all 3"));
        assert!(check(&graph, &[(1, 9)], 3).contains("vertex 9 is outside 1..3"));
    }

    #[test]
    fn checking_accepts_what_it_should() {
        let tree = spec(EdgeKind::Tree, false, true, false);
        let digraph = spec(EdgeKind::Graph, true, false, false);
        assert_eq!(check_edges(&tree, &[(1, 2), (3, 2), (2, 4)], 1, 4), None);
        assert_eq!(check_edges(&digraph, &[(1, 2), (2, 1)], 1, 2), None, "both directions are different edges");
        assert_eq!(check_edges(&tree, &[], 1, 1), None);
        assert_eq!(check_edges(&spec(EdgeKind::Graph, false, false, false), &[], 1, 3), None);
        // Vertices may be numbered from 0.
        assert_eq!(check_edges(&tree, &[(0, 1), (1, 2)], 0, 3), None);
    }

    #[test]
    fn the_number_of_edges_that_goes_with_a_kind_of_list() {
        assert_eq!(count_range(&spec(EdgeKind::Tree, false, true, false), "n"), "n-1..n-1");
        assert_eq!(count_range(&spec(EdgeKind::Graph, false, false, false), "n"), "0..n*(n-1)/2");
        assert_eq!(count_range(&spec(EdgeKind::Graph, false, true, false), "n"), "n-1..n*(n-1)/2");
        assert_eq!(count_range(&spec(EdgeKind::Graph, true, false, false), "k"), "0..k*(k-1)");
    }
}
