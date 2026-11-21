//! Integration tests for sigmaos-rebuild-order.

use sigmaos_rebuild_order::{SyncDb, SyncPkg, rebuild_order};

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

fn make_db(pkgs: Vec<SyncPkg>) -> SyncDb {
    let mut d = SyncDb::default();
    for p in pkgs {
        d.by_name.insert(p.name.clone(), d.packages.len());
        d.packages.push(p);
    }
    d
}

#[test]
fn test_diamond_dependency_order() {
    // app -> {liba, libb}; liba -> base; libb -> base.
    let d = make_db(vec![
        pkg("app", &["liba", "libb"], &[]),
        pkg("liba", &["base"], &[]),
        pkg("libb", &["base"], &[]),
        pkg("base", &[], &[]),
    ]);
    let order = rebuild_order(&d, &["base".into()], false, false).unwrap();
    let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
    assert!(pos("base") < pos("liba"));
    assert!(pos("base") < pos("libb"));
    assert!(pos("liba") < pos("app") || pos("libb") < pos("app"));
    assert!(order.contains(&"app".to_string()));
}

#[test]
fn test_multiple_inputs() {
    let d = make_db(vec![
        pkg("a", &["shared"], &[]),
        pkg("b", &["shared"], &[]),
        pkg("shared", &[], &[]),
    ]);
    let order = rebuild_order(&d, &["shared".into()], false, false).unwrap();
    let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
    assert!(pos("shared") < pos("a"));
    assert!(pos("shared") < pos("b"));
}

#[test]
fn test_provides_resolution() {
    // `virtual-thing` is provided by `real-pkg`; a package depending on the
    // virtual name must be pulled into real-pkg's rebuild order.
    let d = make_db(vec![
        pkg("consumer", &["virtual-thing"], &[]),
        pkg("real-pkg", &[], &["virtual-thing"]),
    ]);
    let order = rebuild_order(&d, &["real-pkg".into()], false, false).unwrap();
    assert!(order.iter().any(|n| n == "real-pkg"));
    assert!(order.iter().any(|n| n == "consumer"));
}

#[test]
fn test_makedepends_counted() {
    let mut build_dep = pkg("tool", &[], &[]);
    build_dep.makedepends = vec!["helper".into()];
    let d = make_db(vec![
        pkg("pkg", &[], &[]),
        build_dep,
        pkg("helper", &[], &[]),
    ]);
    let order = rebuild_order(&d, &["helper".into()], false, false).unwrap();
    assert!(order.contains(&"tool".to_string()));
    assert!(order.contains(&"helper".to_string()));
}
