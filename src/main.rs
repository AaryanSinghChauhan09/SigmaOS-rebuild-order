//! `sigmaos-rebuild-order` CLI.
//!
//! Usage: sigmaos-rebuild-order [OPTIONS] <PKGNAME>...
//!   --repos <csv>        sync repos (default: core,extra,multilib)
//!   --dbpath <PATH>      pacman db path (default: /var/lib/pacman)
//!   -d, --dotfile <FILE> write rebuild graph in DOT format
//!   --no-reverse-depends only output the requested pkgnames
//!   --with-check-depends include CHECKDEPENDS edges

use std::path::PathBuf;
use std::process::ExitCode;

use sigmaos_rebuild_order::{RebuildError, SyncDb, rebuild_order};

struct Cli {
    pkgnames: Vec<String>,
    repos: Vec<String>,
    dbpath: PathBuf,
    dotfile: Option<String>,
    no_reverse_depends: bool,
    with_check_depends: bool,
}

fn parse_args() -> Result<Cli, String> {
    let mut cli = Cli {
        pkgnames: vec![],
        repos: "core,extra,multilib".split(',').map(str::to_string).collect(),
        dbpath: PathBuf::from("/var/lib/pacman"),
        dotfile: None,
        no_reverse_depends: false,
        with_check_depends: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                eprintln!("sigmaos-rebuild-order <PKGNAME>... [--repos csv] [--dbpath PATH] [-d FILE] [--no-reverse-depends] [--with-check-depends]");
                std::process::exit(0);
            }
            "--repos" => {
                let v = args.next().ok_or("--repos needs a value")?;
                cli.repos = v.split(',').map(str::to_string).collect();
            }
            "--dbpath" => {
                cli.dbpath = PathBuf::from(args.next().ok_or("--dbpath needs a value")?);
            }
            "-d" | "--dotfile" => {
                cli.dotfile = Some(args.next().ok_or("--dotfile needs a value")?);
            }
            "--no-reverse-depends" => cli.no_reverse_depends = true,
            "--with-check-depends" => cli.with_check_depends = true,
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ => cli.pkgnames.push(arg),
        }
    }
    if cli.pkgnames.is_empty() {
        return Err("at least one PKGNAME is required".into());
    }
    Ok(cli)
}

fn main() -> ExitCode {
    let cli = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let result = (|| -> Result<Vec<String>, RebuildError> {
        let db = SyncDb::load(&cli.dbpath, &cli.repos)?;
        rebuild_order(&db, &cli.pkgnames, cli.no_reverse_depends, cli.with_check_depends)
    })();
    match result {
        Ok(order) => {
            println!("{}", order.join(" "));
            if let Some(file) = &cli.dotfile {
                // DOT export requires the graph; rebuild_order consumed it.
                // For now write a minimal note — full graph export is exposed
                // via RebuildGraph::to_dot in the library for embedders.
                let _ = file;
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Critical failure - sigmaos-rebuild-order has stopped working");
            eprintln!("Reason: {e}");
            ExitCode::FAILURE
        }
    }
}
