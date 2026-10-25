//! `git kb check`: what in the pages' front matter git-kb cannot make sense of, said one problem a line. Front
//! matter that is not YAML it reads, a doctype the settings do not have, a status its doctype does not list, a
//! number two pages of a doctype share, as when two branches both took the next one, a `replaces` that names no
//! page, and a citation of a source the page does not have. Pages are shown whatever it says.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::build::Builder;
use crate::config::Config;
use crate::front::Value;
use crate::nav::Pages;
use crate::render::Renderer;
use crate::shell::{Links, Shell};

/// A problem with a page: which, and what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub page: PathBuf,
    pub said: String,
}

/// Every problem with the front matter of the site's pages, in the folder as it is, by page.
///
/// # Errors
///
/// When the folder cannot be read.
pub fn check(config: &Config) -> anyhow::Result<Vec<Problem>> {
    let renderer = Renderer::new();
    let files = Builder::files(&config.root, None)?;
    let read = |f: &Path| fs::read_to_string(config.root.join(f)).ok();
    let pages = Pages::new(&files, &renderer, read);
    let shell = Shell::new(String::new(), Links::Built).typing(config.doctypes.clone());
    let mut problems = vec![];
    let mut numbers: BTreeMap<(&str, u64), Vec<&Path>> = BTreeMap::new();
    for file in &files {
        let Some(front) = pages.front(file) else {
            continue;
        };
        let mut problem = |said: String| {
            problems.push(Problem {
                page: file.clone(),
                said,
            });
        };
        if let Some(error) = &front.error {
            problem(format!("its front matter cannot be read: {error}"));
            continue;
        }
        let Some(id) = front.doctype() else {
            continue;
        };
        let Some(doctype) = config.doctypes.get(id) else {
            problem(format!("no doctype {id} in {}", Config::FILE));
            continue;
        };
        if let Some(status) = front.text("status")
            && !doctype.statuses.is_empty()
            && !doctype.statuses.iter().any(|s| s == status)
        {
            problem(format!(
                "status {status} is not one of {id}'s: {}",
                doctype.statuses.join(", ")
            ));
        }
        if let Some(number) = front.number() {
            numbers.entry((id, number)).or_default().push(file);
        }
        if let Some(Value::Text(replaces)) = front.get("replaces")
            && pages.named(replaces).is_none()
        {
            problem(format!("replaces {replaces}, which is no page of the site"));
        }
        if doctype.citations
            && let Some(text) = read(file)
        {
            let cited = shell.cited(&renderer.render(&text).html, Some(front));
            for missing in cited.split("class=\"cite broken\" href=\"#ref-").skip(1) {
                let key = missing.split('"').next().unwrap_or_default();
                problem(format!("cites {key}, which is not among its sources"));
            }
        }
    }
    for ((id, number), mut shared) in numbers {
        shared.sort();
        if shared.len() > 1 {
            let others: Vec<String> = shared.iter().map(|p| p.display().to_string()).collect();
            for page in &shared {
                problems.push(Problem {
                    page: page.to_path_buf(),
                    said: format!("{id} number {number} is shared by {}", others.join(", ")),
                });
            }
        }
    }
    problems.sort_by(|a, b| a.page.cmp(&b.page));
    problems.dedup();
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DoctypeConfig;

    #[test]
    fn what_cannot_be_made_sense_of_is_said_page_by_page() {
        let dir = tempfile::tempdir().unwrap();
        let files = [
            (
                "a.md",
                "---\ndoctype: design\nnumber: 1\nstatus: shipped\n---\n# A\n",
            ),
            (
                "b.md",
                "---\ndoctype: design\nnumber: 1\nreplaces: nowhere.md\n---\n# B\n",
            ),
            ("c.md", "---\ndoctype: memo\n---\n# C\n"),
            ("d.md", "---\nstatus: [unclosed\n---\n# D\n"),
            (
                "e.md",
                "---\ndoctype: survey\nsources:\n  k: \"K\"\n---\n# E\n\nSee [@k] and [@gone].\n",
            ),
            (
                "f.md",
                "---\ndoctype: design\nnumber: 2\nstatus: draft\nreplaces: A\n---\n# F\n",
            ),
            ("g.md", "# No front matter\n"),
        ];
        for (name, text) in files {
            fs::write(dir.path().join(name), text).unwrap();
        }
        let design = DoctypeConfig {
            statuses: vec!["draft".into()],
            ..DoctypeConfig::default()
        };
        let survey = DoctypeConfig {
            citations: true,
            ..DoctypeConfig::default()
        };
        let config = Config {
            root: fs::canonicalize(dir.path()).unwrap(),
            doctypes: BTreeMap::from([("design".into(), design), ("survey".into(), survey)]),
            ..Config::default()
        };
        let said: Vec<String> = check(&config)
            .unwrap()
            .iter()
            // What the YAML reader says of front matter it cannot read is its own.
            .map(|p| {
                format!(
                    "{}: {}",
                    p.page.display(),
                    p.said.split(": while").next().unwrap_or_default()
                )
            })
            .collect();
        assert_eq!(
            said,
            [
                "a.md: status shipped is not one of design's: draft",
                "a.md: design number 1 is shared by a.md, b.md",
                "b.md: replaces nowhere.md, which is no page of the site",
                "b.md: design number 1 is shared by a.md, b.md",
                "c.md: no doctype memo in git-kb.toml",
                "d.md: its front matter cannot be read",
                "e.md: cites gone, which is not among its sources",
            ]
        );
    }
}
