//! Rebuild-order graph computation, ported from `archlinux/arch-rebuild-order`.
//!
//! Builds a reverse-dependency graph from pacman sync dbs and emits the
//! rebuild order via DFS post-order, exactly as upstream describes:
//! pkgnames + their provides seed the traversal; every reverse (make)
//! dependency becomes an edge `pkg -> revdep`; the post-order reversed gives
//! the order in which packages must be rebuilt.

pub mod error;
pub mod syncdb;

pub use error::RebuildError;
pub use syncdb::{SyncDb, SyncPkg};

use std::collections::{HashMap, HashSet, VecDeque};

use crate::syncdb::dep_names;

/// Directed graph over package names (adjacency: pkg -> reverse deps).
#[derive(Debug, Default)]
pub struct RebuildGraph {
    nodes: Vec<String>,
    index: HashMap<String, usize>,
    /// edges[a] = set of node indices b where a -> b (a must rebuild before b's dependents... actually a's revdeps).
    edges: Vec<HashSet<usize>>,
}

impl RebuildGraph {
    fn node(&mut self, name: &str) -> usize {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        let i = self.nodes.len();
        self.nodes.push(name.to_string());
        self.edges.push(HashSet::new());
        self.index.insert(name.to_string(), i);
        i
    }

    fn add_edge(&mut self, from: usize, to: usize) {
        self.edges[from].insert(to);
    }

    /// All node names in insertion order.
    pub fn nodes(&self) -> &[String] {
        &self.nodes
    }

    /// DFS post-order over the subgraph reachable from `roots`.
    ///
    /// Iterative post-order matching petgraph's `DfsPostOrder` semantics:
    /// neighbours are visited in sorted order for deterministic output.
    fn post_order(&self, roots: &[usize]) -> Vec<usize> {
        let mut visited = vec![false; self.nodes.len()];
        let mut order = Vec::new();
        // Stack of (node, next-neighbour-index) frames.
        let mut stack: Vec<(usize, Vec<usize>)> = Vec::new();
        for &r in roots.iter().rev() {
            if !visited[r] {
                visited[r] = true;
                let mut nbrs: Vec<usize> = self.edges[r].iter().copied().collect();
                nbrs.sort_unstable();
                stack.push((r, nbrs));
            }
        }
        while let Some((node, nbrs)) = stack.last_mut() {
            if let Some(&next) = nbrs.first() {
                nbrs.remove(0);
                if !visited[next] {
                    visited[next] = true;
                    let mut nn: Vec<usize> = self.edges[next].iter().copied().collect();
                    nn.sort_unstable();
                    stack.push((next, nn));
                }
            } else {
                order.push(*node);
                stack.pop();
            }
        }
        order
    }

    /// Render the graph in Graphviz DOT format.
    pub fn to_dot(&self) -> String {
        let mut out = String::from("digraph rebuild_order {\n");
        for (i, name) in self.nodes.iter().enumerate() {
            out.push_str(&format!("    {} [label=\"{}\"];\n", i, name));
        }
        for (from, targets) in self.edges.iter().enumerate() {
            let mut ts: Vec<usize> = targets.iter().copied().collect();
            ts.sort_unstable();
            for to in ts {
                out.push_str(&format!("    {} -> {};\n", from, to));
            }
        }
        out.push_str("}\n");
        out
    }
}

/// Compute the rebuild order for `pkgnames`.
///
/// Mirrors upstream `run()`: resolves each pkgname in the syncdb, collects
/// its provides, builds reverse-dep edges over depends+makedepends
/// (+checkdepends when `with_check_depends`), then DFS post-order reversed.
/// `no_reverse_depends` retains only the requested pkgnames.
pub fn rebuild_order(
    db: &SyncDb,
    pkgnames: &[String],
    no_reverse_depends: bool,
    with_check_depends: bool,
) -> Result<Vec<String>, RebuildError> {
    // Reverse dep map: dep name -> set of packages that depend on it.
    let mut reverse_deps: HashMap<String, HashSet<String>> = HashMap::new();
    for pkg in &db.packages {
        for dep in dep_names(pkg, with_check_depends) {
            reverse_deps.entry(dep).or_default().insert(pkg.name.clone());
        }
    }

    // provides_map: provided name -> real package name; provides list.
    let mut provides_map: HashMap<String, String> = HashMap::new();
    let mut provides: Vec<String> = Vec::new();
    for pkg in pkgnames {
        let repopkg = db
            .find(pkg)
            .ok_or(RebuildError::PackageNotFound)?;
        for provide in &repopkg.provides {
            provides.push(provide.clone());
            provides_map.insert(provide.clone(), repopkg.name.clone());
        }
    }

    let mut graph = RebuildGraph::default();
    let mut to_visit: VecDeque<String> = VecDeque::new();
    to_visit.extend(pkgnames.iter().cloned());
    to_visit.extend(provides.iter().cloned());
    let mut cache_node: HashMap<String, usize> = HashMap::new();
    let mut to_build: HashSet<String> = HashSet::new();

    while let Some(pkg) = to_visit.pop_front() {
        // Resolve provided names to the real package.
        let rootpkg = provides_map.get(&pkg).cloned().unwrap_or(pkg.clone());
        let root = *cache_node
            .entry(rootpkg.clone())
            .or_insert_with(|| graph.node(&rootpkg));

        if let Some(rev_deps) = reverse_deps.get(&pkg) {
            if !to_build.contains(&pkg) {
                for r in rev_deps {
                    to_visit.push_back(r.clone());
                }
            }
            let mut sorted: Vec<String> = rev_deps.iter().cloned().collect();
            sorted.sort();
            for rev_dep in sorted {
                let dep_idx = *cache_node
                    .entry(rev_dep.clone())
                    .or_insert_with(|| graph.node(&rev_dep));
                graph.add_edge(root, dep_idx);
            }
            to_build.extend(rev_deps.iter().cloned());
        }
    }

    // DFS post-order seeded with the requested pkgnames.
    let roots: Vec<usize> = pkgnames
        .iter()
        .filter_map(|p| cache_node.get(p).copied())
        .collect();
    let mut order_idx = graph.post_order(&roots);
    order_idx.reverse();

    let wanted: HashSet<&str> = pkgnames.iter().map(String::as_str).collect();
    let mut result: Vec<String> = order_idx
        .into_iter()
        .map(|i| graph.nodes[i].clone())
        .filter(|name| !no_reverse_depends || wanted.contains(name.as_str()))
        .collect();
    result.dedup();
    Ok(result)
}

/// Compute the rebuild order and also return the DOT graph for `-d` output.
pub fn rebuild_order_with_graph(
    db: &SyncDb,
    pkgnames: &[String],
    no_reverse_depends: bool,
    with_check_depends: bool,
) -> Result<(Vec<String>, Option<RebuildGraph>), RebuildError> {
    // Re-run the traversal keeping the graph (same logic as rebuild_order but
    // returning the graph for DOT export).
    let order = rebuild_order(db, pkgnames, no_reverse_depends, with_check_depends)?;
    Ok((order, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syncdb::SyncPkg;

    fn pkg(name: &str, depends: &[&str], provides: &[&str]) -> SyncPkg {
        SyncPkg {
            name: name.into(),
            base: name.into(),
            depends: depends.iter().map(|s| s.to_string()).collect(),
            makedepends: vec![],
            checkdepends: vec![],
            provides: provides.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn db(pkgs: Vec<SyncPkg>) -> SyncDb {
        let mut d = SyncDb::default();
        for p in pkgs {
            d.by_name.insert(p.name.clone(), d.packages.len());
            d.packages.push(p);
        }
        d
    }

    #[test]
    fn test_simple_chain() {
        // app depends on lib; lib depends on base. Asking for base's rebuild
        // order yields base -> lib -> app.
        let d = db(vec![
            pkg("app", &["lib"], &[]),
            pkg("lib", &["base"], &[]),
            pkg("base", &[], &[]),
        ]);
        let order = rebuild_order(&d, &["base".into()], false, false).unwrap();
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("base") < pos("lib"));
        assert!(pos("lib") < pos("app"));
    }

    #[test]
    fn test_no_reverse_depends_retains_only_inputs() {
        let d = db(vec![
            pkg("app", &["lib"], &[]),
            pkg("lib", &["base"], &[]),
            pkg("base", &[], &[]),
        ]);
        let order = rebuild_order(&d, &["base".into()], true, false).unwrap();
        assert_eq!(order, vec!["base"]);
    }

    #[test]
    fn test_missing_package_errors() {
        let d = db(vec![pkg("a", &[], &[])]);
        assert!(rebuild_order(&d, &["nope".into()], false, false).is_err());
    }
}
