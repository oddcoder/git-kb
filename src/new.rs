//! `git kb new <doctype> "Title"`: a new page of a doctype, from its template, in a folder of the site. The
//! template's `{title}`, `{number}`, `{date}` and `{author}` are filled in: the title given, the doctype's next
//! number, the highest any page of it has, in the folder as it is, plus one, today, and who writes, as git says. It
//! writes that one file and nothing else, nor commits it; a number two branches both took is for review to catch.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::build::Builder;
use crate::config::Config;
use crate::render::Renderer;

/// What a new page is: its doctype, its title, the folder it goes in, and who writes it, on what day.
#[derive(Debug, Clone)]
pub struct New<'a> {
    pub doctype: &'a str,
    pub title: &'a str,
    pub folder: &'a Path,
    pub author: &'a str,
    pub date: &'a str,
}

/// A page with only front matter and a title, for a doctype with no template.
const BARE: &str =
    "---\ndoctype: {doctype}\nnumber: {number}\nauthors: [{author}]\n---\n# {title}\n";

/// Writes a new page, and says where.
///
/// # Errors
///
/// When the settings have no such doctype, its template cannot be read, the folder is not in the site, or a page
/// of the name the title gives is there already.
pub fn new_page(config: &Config, new: &New) -> anyhow::Result<PathBuf> {
    let Some(doctype) = config.doctypes.get(new.doctype) else {
        let ids: Vec<&str> = config.doctypes.keys().map(String::as_str).collect();
        anyhow::bail!(
            "no doctype {} in {}; it has {}",
            new.doctype,
            Config::FILE,
            if ids.is_empty() {
                "none".into()
            } else {
                ids.join(", ")
            }
        );
    };
    let folder = fs::canonicalize(new.folder).with_context(|| new.folder.display().to_string())?;
    anyhow::ensure!(
        folder.starts_with(&config.root),
        "{} is not in the site, {}",
        folder.display(),
        config.root.display()
    );
    let template = match &doctype.template {
        Some(file) => {
            fs::read_to_string(file).with_context(|| format!("its template, {}", file.display()))?
        }
        None => BARE.to_string(),
    };
    let file = folder.join(format!("{}.md", slug(new.title)));
    anyhow::ensure!(!file.exists(), "{} is there already", file.display());
    let number = next_number(&config.root, new.doctype)?;
    let page = template
        .replace("{doctype}", new.doctype)
        .replace("{title}", new.title)
        .replace("{number}", &number.to_string())
        .replace("{date}", new.date)
        .replace("{author}", new.author);
    fs::write(&file, page).with_context(|| file.display().to_string())?;
    Ok(file)
}

/// A doctype's next number: the highest any page of it has, in a site's folder as it is, plus one.
///
/// # Errors
///
/// When the folder cannot be read.
pub fn next_number(root: &Path, doctype: &str) -> anyhow::Result<u64> {
    let renderer = Renderer::new();
    let highest = Builder::files(root, None)?
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")))
        .filter_map(|f| fs::read_to_string(root.join(f)).ok())
        .filter_map(|text| renderer.front(&text))
        .filter(|front| front.doctype() == Some(doctype))
        .filter_map(|front| front.number())
        .max()
        .unwrap_or(0);
    Ok(highest + 1)
}

/// A title as a file's name: its letters and digits, lower case, with a `-` for each run of anything else.
fn slug(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "untitled".into()
    } else {
        slug.into()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::DoctypeConfig;

    fn site(files: &[(&str, &str)]) -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let file = dir.path().join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }
        let root = fs::canonicalize(dir.path().join("docs")).unwrap();
        let design = DoctypeConfig {
            template: Some(dir.path().join("templates/design.md")),
            ..DoctypeConfig::default()
        };
        let config = Config {
            root,
            doctypes: BTreeMap::from([
                ("design".into(), design),
                ("guide".into(), DoctypeConfig::default()),
            ]),
            ..Config::default()
        };
        (dir, config)
    }

    #[test]
    fn a_new_page_is_its_template_filled_with_the_next_number_of_its_doctype() {
        let (_dir, config) = site(&[
            (
                "templates/design.md",
                "---\ndoctype: design\nnumber: {number}\nauthors: [{author}]\ncreated: {date}\n---\n# {title}\n",
            ),
            ("docs/a.md", "---\ndoctype: design\nnumber: 4\n---\n# A\n"),
            (
                "docs/sub/b.md",
                "---\ndoctype: design\nnumber: 9\n---\n# B\n",
            ),
            ("docs/c.md", "---\ndoctype: guide\nnumber: 30\n---\n# C\n"),
        ]);
        let new = New {
            doctype: "design",
            title: "Cache-Aware Tiling!",
            folder: &config.root.join("sub"),
            author: "Ada",
            date: "2026-10-25",
        };
        let file = new_page(&config, &new).unwrap();
        assert_eq!(file, config.root.join("sub/cache-aware-tiling.md"));
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "---\ndoctype: design\nnumber: 10\nauthors: [Ada]\ncreated: 2026-10-25\n---\n# Cache-Aware Tiling!\n"
        );
        assert!(
            new_page(&config, &new).is_err(),
            "not over a page there already"
        );
        let guide = New {
            doctype: "guide",
            title: "First Steps",
            folder: &config.root,
            ..new
        };
        let file = new_page(&config, &guide).unwrap();
        assert!(
            fs::read_to_string(file)
                .unwrap()
                .starts_with("---\ndoctype: guide\nnumber: 31\n"),
            "each doctype counts its own; one without a template is front matter and a title"
        );
    }

    #[test]
    fn a_new_page_needs_a_doctype_the_settings_have_and_a_folder_of_the_site() {
        let (dir, config) = site(&[("docs/a.md", "# A\n")]);
        let new = New {
            doctype: "memo",
            title: "T",
            folder: &config.root,
            author: "Ada",
            date: "2026-10-25",
        };
        let error = new_page(&config, &new).unwrap_err().to_string();
        assert!(error.contains("design, guide"), "{error}");
        let outside = New {
            doctype: "guide",
            folder: dir.path(),
            ..new
        };
        assert!(new_page(&config, &outside).is_err());
    }
}
