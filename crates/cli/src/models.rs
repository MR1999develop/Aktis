//! What the AI commands download the first time they need it, and
//! `rne models` / `rne models --download`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;

use ai_models::{Feature, Pack};

use crate::BoxError;
use crate::args::{Selection, Step};

/// The AI features a selection uses.
pub fn features_of(sel: &Selection) -> Vec<Feature> {
    let mut out = Vec::new();
    for (_, step) in &sel.steps {
        let feature = match step {
            Step::Subject => Some(Feature::Subject),
            Step::Detect(_) => Some(Feature::People),
            Step::Parts(_) => Some(Feature::PersonParts),
            Step::Object { .. } => Some(Feature::Objects),
            Step::Brush { .. } => None,
        };
        if let Some(f) = feature
            && !out.contains(&f)
        {
            out.push(f);
        }
    }
    out
}

/// The packs `features` need, each once.
fn packs_for(features: &[Feature]) -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = Vec::new();
    for f in features {
        for id in ai_models::needs(*f) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    ids
}

/// Makes sure what `features` need is installed. When something is missing
/// and the command runs in a terminal, asks whether to download it now;
/// otherwise says how to.
///
/// # Errors
/// When something is missing and is not downloaded.
pub fn ensure(features: &[Feature]) -> Result<(), BoxError> {
    if features.is_empty() {
        return Ok(());
    }
    let dir = ai_models::models_dir();
    let missing = ai_models::missing(&packs_for(features), &dir);
    if missing.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = features.iter().map(|f| f.label()).collect();
    eprintln!("rne: {} needs files that are not installed yet:", names.join(", "));
    describe(&missing, &dir);
    let cli: Vec<&str> = features.iter().map(|f| f.cli_name()).collect();
    if !std::io::stdin().is_terminal() {
        return Err(format!("not installed; download with `rne models --download {}`", cli.join(" ")).into());
    }
    if !confirm("Download now?")? {
        return Err(format!("not downloaded; later: `rne models --download {}`", cli.join(" ")).into());
    }
    run(&missing, &dir)
}

/// `rne models --download <what>...`.
///
/// # Errors
/// An unknown name, a refused confirmation or a failed download.
pub fn download_cmd(what: &[String], yes: bool) -> Result<(), BoxError> {
    let catalog = ai_models::catalog();
    let mut ids: Vec<&'static str> = Vec::new();
    for name in what {
        let found: Vec<&'static str> = if name.eq_ignore_ascii_case("all") {
            catalog.iter().map(|p| p.id).collect()
        } else if let Some(f) = Feature::from_cli_name(name) {
            ai_models::needs(f)
        } else if let Some(p) = catalog.iter().find(|p| p.id.eq_ignore_ascii_case(name)) {
            vec![p.id]
        } else {
            let tools: Vec<&str> = Feature::ALL.iter().map(|f| f.cli_name()).collect();
            let packs: Vec<&str> = catalog.iter().map(|p| p.id).collect();
            return Err(format!(
                "'{name}' is neither a tool ({}), a pack ({}) nor all",
                tools.join(", "),
                packs.join(", ")
            )
            .into());
        };
        for id in found {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    let dir = ai_models::models_dir();
    let missing = ai_models::missing(&ids, &dir);
    if missing.is_empty() {
        println!("Everything asked for is already installed in {}", dir.display());
        return Ok(());
    }
    eprintln!("rne: to download:");
    describe(&missing, &dir);
    if !yes && std::io::stdin().is_terminal() && !confirm("Download now?")? {
        return Err("not downloaded".into());
    }
    run(&missing, &dir)
}

/// Lists packs with size and licence, and the folder.
fn describe(packs: &[Pack], dir: &Path) {
    for p in packs {
        eprintln!("  {:<44} {:>14}  {}", p.name, p.size_text(), p.license);
    }
    eprintln!("  into {}", dir.display());
}

/// Asks a yes/no question on the terminal; Enter means yes.
fn confirm(question: &str) -> Result<bool, BoxError> {
    eprint!("{question} [Y/n] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    let a = answer.trim();
    Ok(a.is_empty() || a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes") || a.eq_ignore_ascii_case("s"))
}

/// Downloads `packs` into `dir`, one after the other with a progress line.
/// A pack that fails does not stop the others; the failures are listed at the end.
fn run(packs: &[Pack], dir: &Path) -> Result<(), BoxError> {
    let cancel = AtomicBool::new(false);
    let mut failures = Vec::new();
    for (n, pack) in packs.iter().enumerate() {
        eprintln!("rne: [{}/{}] {} ({})", n + 1, packs.len(), pack.name, pack.size_text());
        let mut shown = (String::new(), u64::MAX);
        let result = ai_models::download(
            std::slice::from_ref(pack),
            dir,
            &mut |p| {
                let mb = p.done / 1_000_000;
                if shown.0 == p.file && shown.1 == mb {
                    return;
                }
                if shown.0 != p.file && !shown.0.is_empty() {
                    eprintln!();
                }
                shown = (p.file.clone(), mb);
                let size = match (p.total, p.fraction()) {
                    (Some(t), Some(f)) => {
                        format!("{} / {} ({:.0}%)", ai_models::megabytes(p.done), ai_models::megabytes(t), f * 100.0)
                    }
                    _ => ai_models::megabytes(p.done),
                };
                eprint!("\r      {}  {size}      ", p.file);
                let _ = std::io::stderr().flush();
            },
            &cancel,
        );
        if !shown.0.is_empty() {
            eprintln!();
        }
        if let Err(e) = result {
            eprintln!("rne:   failed: {e}");
            failures.push(format!("{}: {e}", pack.name));
        }
    }
    if failures.is_empty() {
        eprintln!("rne: installed in {}", dir.display());
        Ok(())
    } else {
        Err(format!("{} of {} not installed:\n  {}", failures.len(), packs.len(), failures.join("\n  ")).into())
    }
}

/// What `rne models` prints: every pack with its state.
///
/// # Errors
/// Never in practice; kept fallible for the command table.
pub fn list(verify: bool, runtime_line: &str) -> Result<(), BoxError> {
    let dir = ai_models::models_dir();
    println!("models folder  {}", dir.display());
    println!("ONNX Runtime   {runtime_line}");
    let mut all_ready = true;
    for p in ai_models::catalog() {
        let missing = ai_models::missing_files(&p, &dir);
        let damaged = if verify && missing.is_empty() { ai_models::damaged_files(&p, &dir) } else { Vec::new() };
        let state = if !missing.is_empty() {
            "missing"
        } else if !damaged.is_empty() {
            "damaged (wrong checksum)"
        } else if verify {
            "ready, checked"
        } else {
            "ready"
        };
        let optional = if p.optional { ", optional" } else { "" };
        println!("\n{}  {}  [{}, {}{optional}]  {state}", p.id, p.name, p.license, p.size_text());
        println!("  {}", p.purpose);
        println!("  from {}", p.source);
        if !p.optional && (!missing.is_empty() || !damaged.is_empty()) {
            all_ready = false;
        }
    }
    if !all_ready {
        let tools: Vec<&str> = Feature::ALL.iter().map(|f| f.cli_name()).collect();
        println!("\nInstall with `rne models --download <{}|pack id|all>`.", tools.join("|"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Op;

    #[test]
    fn a_selection_needs_the_models_of_its_parts_and_brushes_need_none() {
        let sel = Selection {
            steps: vec![
                (Op::Add, Step::Detect(0)),
                (Op::Subtract, Step::Brush { radius: 0.1, path: vec![(0.5, 0.5)] }),
                (Op::Add, Step::Detect(16)),
                (Op::Intersect, Step::Subject),
            ],
            ..Selection::default()
        };
        assert_eq!(features_of(&sel), vec![Feature::People, Feature::Subject]);
        let only_brush =
            Selection { steps: vec![(Op::Add, Step::Brush { radius: 0.1, path: vec![] })], ..Selection::default() };
        assert!(features_of(&only_brush).is_empty());
        let ids = packs_for(&[Feature::People, Feature::PersonParts]);
        assert_eq!(ids.iter().filter(|id| **id == "onnx-runtime").count(), 1, "{ids:?}");
    }
}
