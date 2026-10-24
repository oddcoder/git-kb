//! Plugins: what renders the blocks that Markdown alone does not, such as `` ```tikz `` diagrams. Each is a folder
//! with a plugin.toml; the built-in ones are in git-kb/plugins and in the binary, and a repository can add its own
//! by path in git-kb.toml. Only the plugins git-kb.toml names are used; without a git-kb.toml, every built-in one.
//!
//! ```toml
//! name = "mermaid"
//! description = "Mermaid diagrams"
//! blocks = ["mermaid"]         # the fence languages it renders
//! figure = true                # whether its blocks are figures: with their source under a bar, zoomable
//! highlight = "text"           # the language their source is highlighted as, under the bar
//!
//! # Rendered in the browser: the HTML each block becomes, which the plugin's scripts then draw.
//! stage = '<pre class="mermaid">{source.html}</pre>'
//! head = ['<script src="https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.min.js" defer></script>']
//! scripts = ["mermaid.js"]     # its own files, loaded after the page's, on pages that use it
//! styles = ["mermaid.css"]
//!
//! # Or rendered by git-kb, when it serves and when it builds: a command given the source on its input, whose
//! # output, SVG or HTML, is the block. Outputs are kept, by their source, so each is made once.
//! command = ["dot", "-Tsvg"]
//!
//! [options]                    # each option and its default; git-kb.toml may set any
//! theme = "neutral"
//! [files]                      # options that name a file, whose content is used; by default the plugin's own
//! preamble = "preamble.tex"
//! ```
//!
//! In stage, head and command, `{name}` is an option's value, escaped for HTML; `{source.html}` is the block's
//! source, escaped; `{source.raw}` is the source as it is, for a `<script>` (a block whose source would close the
//! script is shown as code instead); and `{assets}` is where the plugin's own files are served.
//!
//! A plugin's scripts are plain scripts, not modules, so that a built site works from disk. They draw the blocks
//! in `[data-plugin="name"]`, again on the event kb-refresh, after the page is swapped for a fresh copy, and may
//! redraw when the reader changes the look, the data- attributes of `<html>`. A block they have drawn they mark
//! data-rendered, so that a fresh copy of the page keeps it as drawn while its source is the same.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;

use crate::config::Config;
use crate::render::Renderer;

/// A plugin's own files, by name.
type Files = &'static [(&'static str, &'static [u8])];

/// The built-in plugins: each one's name, plugin.toml and files.
const BUILT_IN: &[(&str, &str, Files)] = &[
    (
        "tikz",
        include_str!("../plugins/tikz/plugin.toml"),
        &[
            (
                "tikz.js",
                include_bytes!(concat!(env!("OUT_DIR"), "/plugins/tikz/tikz.js")),
            ),
            (
                "preamble.tex",
                include_bytes!("../plugins/tikz/preamble.tex"),
            ),
        ],
    ),
    (
        "mermaid",
        include_str!("../plugins/mermaid/plugin.toml"),
        &[
            (
                "mermaid.js",
                include_bytes!(concat!(env!("OUT_DIR"), "/plugins/mermaid/mermaid.js")),
            ),
            (
                "mermaid.css",
                include_bytes!("../plugins/mermaid/mermaid.css"),
            ),
        ],
    ),
    (
        "wavedrom",
        include_str!("../plugins/wavedrom/plugin.toml"),
        &[
            (
                "wavedrom.js",
                include_bytes!(concat!(env!("OUT_DIR"), "/plugins/wavedrom/wavedrom.js")),
            ),
            (
                "wavedrom.css",
                include_bytes!("../plugins/wavedrom/wavedrom.css"),
            ),
        ],
    ),
];

/// The built-in plugins' names.
#[must_use]
pub fn built_in_names() -> Vec<&'static str> {
    BUILT_IN.iter().map(|(name, ..)| *name).collect()
}

/// A built-in plugin's description, and what it renders.
#[must_use]
pub fn describe(name: &str) -> String {
    let Some((_, manifest, _)) = BUILT_IN.iter().find(|(n, ..)| *n == name) else {
        return String::new();
    };
    let Ok(m) = toml::from_str::<Manifest>(manifest) else {
        return String::new();
    };
    let blocks: Vec<String> = m.blocks.iter().map(|b| format!("```{b}")).collect();
    format!("{} ({})", m.description, blocks.join(", "))
}

/// A plugin.toml.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    blocks: Vec<String>,
    #[serde(default)]
    figure: bool,
    #[serde(default)]
    highlight: Option<String>,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    head: Vec<String>,
    #[serde(default)]
    scripts: Vec<String>,
    #[serde(default)]
    styles: Vec<String>,
    #[serde(default)]
    command: Option<Vec<String>>,
    #[serde(default)]
    options: BTreeMap<String, toml::Value>,
    #[serde(default)]
    files: BTreeMap<String, String>,
}

/// A plugin, loaded: what it renders, how, and its options' values.
#[derive(Debug)]
pub struct Plugin {
    pub name: String,
    pub description: String,
    pub blocks: Vec<String>,
    pub figure: bool,
    highlight: String,
    stage: Option<String>,
    head: Vec<String>,
    scripts: Vec<String>,
    styles: Vec<String>,
    command: Option<Vec<String>>,
    /// Every option's value, as text: given in git-kb.toml, or the plugin's default; a file option, its content.
    options: BTreeMap<String, String>,
    /// Its own files, served to the browser.
    files: BTreeMap<String, Vec<u8>>,
}

/// The plugins in use.
#[derive(Debug, Default)]
pub struct Plugins {
    list: Vec<Plugin>,
    /// Where commands' outputs are kept.
    cache: Option<PathBuf>,
}

static PLUGINS: OnceLock<Plugins> = OnceLock::new();

/// The plugins in use: those set at start, or none.
pub fn get() -> &'static Plugins {
    PLUGINS.get_or_init(Plugins::default)
}

/// Sets the plugins in use, once, at start.
pub fn set(plugins: Plugins) {
    let _ = PLUGINS.set(plugins);
}

impl Plugins {
    /// The plugins git-kb.toml names, with its options; with no git-kb.toml, every built-in one. Commands' outputs
    /// are kept in `cache`.
    ///
    /// # Errors
    ///
    /// When a plugin cannot be loaded, or two render the same blocks.
    pub fn load(
        config: &Config,
        file: Option<&Path>,
        cache: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let named: Vec<(String, toml::Table)> = if file.is_some() {
            config
                .plugins
                .iter()
                .map(|(name, options)| (name.clone(), options.clone()))
                .collect()
        } else {
            built_in_names()
                .into_iter()
                .map(|name| (name.to_string(), toml::Table::new()))
                .collect()
        };
        let mut list: Vec<Plugin> = vec![];
        for (name, given) in named {
            let plugin = Plugin::load(&name, given, &config.base)
                .with_context(|| format!("plugin {name}"))?;
            for lang in &plugin.blocks {
                if let Some(other) = list.iter().find(|p| p.blocks.contains(lang)) {
                    bail!(
                        "plugins {} and {name} both render ```{lang} blocks; use one",
                        other.name
                    );
                }
            }
            list.push(plugin);
        }
        if let Some(dir) = &cache {
            let _ = fs::create_dir_all(dir);
        }
        Ok(Self { list, cache })
    }

    #[must_use]
    pub fn all(&self) -> &[Plugin] {
        &self.list
    }

    /// The plugin for a fence's language, if one renders it.
    #[must_use]
    pub fn block(&self, lang: &str) -> Option<&Plugin> {
        self.list
            .iter()
            .find(|p| p.blocks.iter().any(|b| b == lang))
    }

    /// Whether a fence's language is a figure's: a diagram, whose source is not prose.
    #[must_use]
    pub fn is_figure(&self, lang: &str) -> bool {
        self.block(lang).is_some_and(|p| p.figure)
    }

    #[must_use]
    pub fn by_name(&self, name: &str) -> Option<&Plugin> {
        self.list.iter().find(|p| p.name == name)
    }

    /// A block as its plugin renders it: a figure, with its source under a bar; or, for a plugin whose blocks are
    /// not figures, the block alone.
    #[must_use]
    pub fn render_block(&self, plugin: &Plugin, source: &str) -> String {
        let stage = self.stage(plugin, source);
        let tag = Renderer::escape(&plugin.name);
        if !plugin.figure {
            return format!(r#"<div class="kb-block" data-plugin="{tag}">{stage}</div>"#);
        }
        let code = Renderer::highlight(&plugin.highlight, source)
            .unwrap_or_else(|| Renderer::escape(source));
        format!(
            concat!(
                r#"<figure class="kb-figure kb-block" data-plugin="{tag}"><div class="kb-stage" title="Click to zoom">{stage}</div>"#,
                r#"<details><summary class="kb-bar"><span class="kb-source">Source</span></summary>"#,
                r#"<pre><code>{code}</code></pre></details></figure>"#
            ),
            tag = tag,
            stage = stage,
            code = code,
        )
    }

    /// What a block's stage holds: its command's output, or the plugin's stage with the source in it.
    fn stage(&self, plugin: &Plugin, source: &str) -> String {
        if let Some(command) = &plugin.command {
            return self.run(plugin, command, source);
        }
        let template = plugin
            .stage
            .as_deref()
            .unwrap_or("<pre>{source.html}</pre>");
        if template.contains("{source.raw}") && source.to_ascii_lowercase().contains("</script") {
            return format!(
                r#"<pre class="kb-error">This block cannot be drawn: its source holds "&lt;/script".</pre><pre>{}</pre>"#,
                Renderer::escape(source)
            );
        }
        fill(template, plugin, source, "")
    }

    /// The tags in `<head>` for the plugins a page uses, by the names in its HTML; `root` is where the site's top is
    /// from the page, "/" when served.
    #[must_use]
    pub fn head(&self, html: &[&str], root: &str) -> String {
        let used = used(html);
        let mut out = String::new();
        for plugin in self.list.iter().filter(|p| used.contains(p.name.as_str())) {
            let assets = format!("{root}_plugins/{}/", plugin.name);
            let name = Renderer::escape(&plugin.name);
            // Each tag is marked as the plugin's, for live.js to load it on a page that comes to use it.
            for tag in &plugin.head {
                let tag = fill(tag, plugin, "", &assets);
                match tag.find(|c: char| c.is_whitespace() || c == '>') {
                    Some(i) if tag.starts_with('<') => {
                        let _ = write!(
                            out,
                            r#"{} data-kb-plugin-head="{name}"{}"#,
                            &tag[..i],
                            &tag[i..]
                        );
                    }
                    _ => out.push_str(&tag),
                }
            }
            for style in &plugin.styles {
                let _ = write!(
                    out,
                    r#"<link rel="stylesheet" href="{}{}" data-kb-plugin="{name}">"#,
                    Renderer::escape(&assets),
                    Renderer::escape(style)
                );
            }
            for script in &plugin.scripts {
                let _ = write!(
                    out,
                    r#"<script src="{}{}" defer data-kb-plugin="{name}"></script>"#,
                    Renderer::escape(&assets),
                    Renderer::escape(script)
                );
            }
        }
        out
    }

    /// One of a plugin's own files, with its type.
    #[must_use]
    pub fn file(&self, name: &str, file: &str) -> Option<(String, &[u8])> {
        let bytes = self.by_name(name)?.files.get(file)?;
        let kind = mime_guess::from_path(file)
            .first_or_octet_stream()
            .to_string();
        Some((kind, bytes))
    }

    /// Every plugin's files, by path under the site, for a build to write.
    #[must_use]
    pub fn every_file(&self) -> Vec<(String, &[u8])> {
        self.list
            .iter()
            .flat_map(|p| {
                p.files.iter().map(move |(file, bytes)| {
                    (format!("_plugins/{}/{file}", p.name), bytes.as_slice())
                })
            })
            .collect()
    }

    /// A command's output for a source, kept by the command, its options and the source, so that each is made once.
    fn run(&self, plugin: &Plugin, command: &[String], source: &str) -> String {
        let args: Vec<String> = command.iter().map(|a| fill_plain(a, plugin)).collect();
        let key = gix::objs::compute_hash(
            gix::hash::Kind::Sha1,
            gix::objs::Kind::Blob,
            format!("{}\0{}\0{source}", plugin.name, args.join("\0")).as_bytes(),
        )
        .map(|id| id.to_string())
        .unwrap_or_default();
        let kept = self
            .cache
            .as_ref()
            .map(|dir| dir.join(format!("{}-{key}.html", plugin.name)));
        if let Some(html) = kept.as_ref().and_then(|path| fs::read_to_string(path).ok()) {
            return html;
        }
        match execute(&args, source) {
            Ok(out) => {
                let html = clean_svg(&out);
                if let Some(path) = &kept {
                    let _ = fs::write(path, &html);
                }
                html
            }
            Err(e) => format!(
                r#"<pre class="kb-error">{}</pre>"#,
                Renderer::escape(&format!(
                    "{}: {e:#}",
                    args.first().map_or("", String::as_str)
                ))
            ),
        }
    }
}

impl Plugin {
    /// A plugin by name, built in or, given a `path`, the repository's own, with the options git-kb.toml gives it.
    fn load(name: &str, mut given: toml::Table, base: &Path) -> anyhow::Result<Self> {
        let path = given
            .remove("path")
            .map(|p| {
                p.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| anyhow!("path must be text"))
            })
            .transpose()?;
        let (manifest, mut files, dir) = if let Some(path) = path {
            let dir = base.join(path);
            let toml = dir.join("plugin.toml");
            let manifest = fs::read_to_string(&toml).with_context(|| toml.display().to_string())?;
            (manifest, BTreeMap::new(), Some(dir))
        } else {
            let (_, manifest, files) =
                BUILT_IN.iter().find(|(n, ..)| *n == name).ok_or_else(|| {
                    anyhow!(
                        "no built-in plugin {name}; give a path to a folder with a plugin.toml{}",
                        match built_in_names().join(", ") {
                            names if names.is_empty() => String::new(),
                            names => format!(", or use one of {names}"),
                        }
                    )
                })?;
            let files = files
                .iter()
                .map(|(f, b)| ((*f).to_string(), b.to_vec()))
                .collect();
            ((*manifest).to_string(), files, None)
        };
        let m: Manifest = toml::from_str(&manifest).context("plugin.toml")?;
        // A plugin of the repository's own serves the files it names.
        if let Some(dir) = &dir {
            for file in m.scripts.iter().chain(&m.styles) {
                let bytes = fs::read(dir.join(file)).with_context(|| file.clone())?;
                files.insert(file.clone(), bytes);
            }
        }
        let mut options = BTreeMap::new();
        for (key, default) in &m.options {
            let value = given.remove(key).unwrap_or_else(|| default.clone());
            options.insert(key.clone(), text(&value));
        }
        for (key, default) in &m.files {
            let content = match given.remove(key) {
                Some(value) => {
                    let path = value
                        .as_str()
                        .ok_or_else(|| anyhow!("{key} must be a path"))?;
                    fs::read(base.join(path)).with_context(|| format!("{key}: {path}"))?
                }
                None => match &dir {
                    Some(dir) => fs::read(dir.join(default)).with_context(|| default.clone())?,
                    None => files
                        .get(default)
                        .cloned()
                        .ok_or_else(|| anyhow!("{default} is missing"))?,
                },
            };
            options.insert(key.clone(), String::from_utf8_lossy(&content).into_owned());
        }
        let command = match given.remove("command") {
            Some(value) => Some(
                value
                    .as_array()
                    .ok_or_else(|| anyhow!("command must be a list"))?
                    .iter()
                    .map(text)
                    .collect(),
            ),
            None => m.command,
        };
        if let Some(unknown) = given.keys().next() {
            let known: Vec<&String> = m.options.keys().chain(m.files.keys()).collect();
            bail!(
                "no option {unknown}; it has {}",
                known
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(Self {
            name: m.name,
            description: m.description,
            blocks: m.blocks,
            figure: m.figure,
            highlight: m.highlight.unwrap_or_default(),
            stage: m.stage,
            head: m.head,
            scripts: m.scripts,
            styles: m.styles,
            command,
            options,
            files,
        })
    }
}

/// A TOML value as text: a list joined by commas.
fn text(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Array(a) => a.iter().map(text).collect::<Vec<_>>().join(","),
        v => v.to_string(),
    }
}

/// A template with its `{placeholders}` filled in.
fn fill(template: &str, plugin: &Plugin, source: &str, assets: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        let Some(j) = rest[i..].find('}') else { break };
        let key = &rest[i + 1..i + j];
        out.push_str(&rest[..i]);
        match key {
            "source.html" => out.push_str(&Renderer::escape(source)),
            "source.raw" => out.push_str(source),
            "assets" => out.push_str(&Renderer::escape(assets)),
            key => match plugin.options.get(key) {
                Some(value) => out.push_str(&Renderer::escape(value)),
                None => out.push_str(&rest[i..=i + j]),
            },
        }
        rest = &rest[i + j + 1..];
    }
    out + rest
}

/// A command's argument with its options filled in, as they are.
fn fill_plain(arg: &str, plugin: &Plugin) -> String {
    plugin.options.iter().fold(arg.to_string(), |a, (k, v)| {
        a.replace(&format!("{{{k}}}"), v)
    })
}

/// Runs a command with the source on its input, for up to a minute, and gives its output.
fn execute(args: &[String], source: &str) -> anyhow::Result<String> {
    let (program, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("the command is empty"))?;
    let mut child = Command::new(program)
        .args(rest)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => anyhow!(
                "it is not installed, or not on the PATH; install it, or set the plugin's command in git-kb.toml"
            ),
            _ => anyhow!("could not run it: {e}"),
        })?;
    let mut input = child.stdin.take().ok_or_else(|| anyhow!("no input"))?;
    let source = source.to_string();
    let writer = std::thread::spawn(move || {
        let _ = input.write_all(source.as_bytes());
    });
    let started = Instant::now();
    while child.try_wait()?.is_none() {
        if started.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            bail!("it took over a minute");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = writer.join();
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("it failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A command's SVG without what comes before `<svg>`, such as an XML declaration, which HTML does not take.
fn clean_svg(out: &str) -> String {
    match out.find("<svg") {
        Some(i) => out[i..].trim_end().to_string(),
        None => out.to_string(),
    }
}

/// The plugins whose blocks are in some HTML, by name.
#[must_use]
pub fn used<'a>(html: &[&'a str]) -> BTreeSet<&'a str> {
    const MARK: &str = r#"data-plugin=""#;
    let mut out = BTreeSet::new();
    for page in html {
        let mut rest = *page;
        while let Some(i) = rest.find(MARK) {
            rest = &rest[i + MARK.len()..];
            if let Some(j) = rest.find('"') {
                out.insert(&rest[..j]);
                rest = &rest[j..];
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A repository's own plugin, in a folder of a temporary one, from its plugin.toml and files, and the folder.
    pub(crate) fn own(manifest: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, toml::Table) {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("kb-plugin");
        fs::create_dir_all(&plugin).unwrap();
        fs::write(plugin.join("plugin.toml"), manifest).unwrap();
        for (name, text) in files {
            fs::write(plugin.join(name), text).unwrap();
        }
        let mut given = toml::Table::new();
        given.insert("path".into(), "kb-plugin".into());
        (dir, given)
    }

    fn load(dir: &tempfile::TempDir, plugins: &[(&str, toml::Table)]) -> anyhow::Result<Plugins> {
        let config = Config {
            plugins: plugins
                .iter()
                .map(|(n, t)| ((*n).to_string(), t.clone()))
                .collect(),
            base: dir.path().to_path_buf(),
            ..Config::default()
        };
        Plugins::load(&config, Some(Path::new("git-kb.toml")), None)
    }

    const DOTS: &str = r#"
        name = "dots"
        description = "Dot diagrams"
        blocks = ["dots"]
        figure = true
        stage = '<pre class="dots" data-size="{size}">{source.html}</pre>'
        head = ['<script src="https://cdn.example.org/dots.js" defer></script>']
        scripts = ["dots.js"]
        styles = ["dots.css"]
        [options]
        size = 3
        [files]
        legend = "legend.txt"
    "#;

    fn dots() -> (tempfile::TempDir, toml::Table) {
        own(
            DOTS,
            &[
                ("dots.js", "draw()"),
                ("dots.css", ".dots {}"),
                ("legend.txt", "A legend"),
            ],
        )
    }

    #[test]
    fn a_figure_s_block_is_drawn_on_its_stage_with_its_source_under_a_bar() {
        let (dir, given) = dots();
        let plugins = load(&dir, &[("dots", given)]).unwrap();
        let plugin = plugins.block("dots").unwrap();
        let html = plugins.render_block(plugin, "a <b> c");
        assert!(
            html.starts_with(
                r#"<figure class="kb-figure kb-block" data-plugin="dots"><div class="kb-stage""#
            ),
            "{html}"
        );
        assert!(
            html.contains(r#"<pre class="dots" data-size="3">a &lt;b&gt; c</pre>"#),
            "{html}"
        );
        assert!(
            html.contains(
                "<summary class=\"kb-bar\"><span class=\"kb-source\">Source</span></summary>"
            ),
            "{html}"
        );
        assert!(plugins.is_figure("dots") && !plugins.is_figure("rust"));
    }

    #[test]
    fn a_plugin_s_options_and_files_are_its_own_unless_git_kb_toml_says() {
        let (dir, mut given) = dots();
        given.insert("size".into(), 7.into());
        fs::write(dir.path().join("mine.txt"), "Ours").unwrap();
        given.insert("legend".into(), "mine.txt".into());
        let plugins = load(&dir, &[("dots", given)]).unwrap();
        let plugin = plugins.by_name("dots").unwrap();
        assert_eq!(plugin.options.get("size").map(String::as_str), Some("7"));
        assert_eq!(
            plugin.options.get("legend").map(String::as_str),
            Some("Ours")
        );
    }

    #[test]
    fn a_plugin_is_refused_an_option_it_does_not_have() {
        let (dir, mut given) = dots();
        given.insert("colour".into(), "red".into());
        let e = load(&dir, &[("dots", given)]).unwrap_err();
        assert!(
            format!("{e:#}").contains("no option colour; it has size, legend"),
            "{e:#}"
        );
    }

    #[test]
    fn two_plugins_may_not_render_the_same_blocks() {
        let (dir, given) = dots();
        let e = load(&dir, &[("dots", given.clone()), ("again", given)]).unwrap_err();
        assert!(
            format!("{e:#}").contains("both render ```dots blocks"),
            "{e:#}"
        );
    }

    #[test]
    fn a_plugin_that_is_neither_built_in_nor_given_a_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let e = load(&dir, &[("nope", toml::Table::new())]).unwrap_err();
        assert!(
            format!("{e:#}").contains("no built-in plugin nope"),
            "{e:#}"
        );
    }

    #[test]
    fn a_page_s_head_loads_only_the_plugins_it_uses() {
        let (dir, given) = dots();
        let plugins = load(&dir, &[("dots", given)]).unwrap();
        assert_eq!(plugins.head(&["<p>No diagrams.</p>"], "/"), "");
        let head = plugins.head(&[r#"<figure data-plugin="dots">"#], "../");
        assert_eq!(
            head,
            concat!(
                r#"<script data-kb-plugin-head="dots" src="https://cdn.example.org/dots.js" defer></script>"#,
                r#"<link rel="stylesheet" href="../_plugins/dots/dots.css" data-kb-plugin="dots">"#,
                r#"<script src="../_plugins/dots/dots.js" defer data-kb-plugin="dots"></script>"#
            )
        );
    }

    #[test]
    fn a_plugin_s_own_files_are_served_and_built() {
        let (dir, given) = dots();
        let plugins = load(&dir, &[("dots", given)]).unwrap();
        let (kind, bytes) = plugins.file("dots", "dots.js").unwrap();
        assert_eq!(
            (kind.as_str(), bytes),
            ("text/javascript", b"draw()".as_slice())
        );
        assert!(
            plugins.file("dots", "legend.txt").is_none(),
            "only its scripts and styles"
        );
        let built: Vec<String> = plugins
            .every_file()
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        assert_eq!(built, ["_plugins/dots/dots.css", "_plugins/dots/dots.js"]);
    }

    #[test]
    fn a_source_that_would_close_its_script_is_shown_as_code() {
        let (dir, given) = own(
            "name = \"raw\"\nblocks = [\"raw\"]\nstage = '<script type=\"text/x-raw\">{source.raw}</script>'\n",
            &[],
        );
        let plugins = load(&dir, &[("raw", given)]).unwrap();
        let plugin = plugins.block("raw").unwrap();
        assert!(
            plugins
                .render_block(plugin, "x")
                .contains(r#"<script type="text/x-raw">x</script>"#)
        );
        let html = plugins.render_block(plugin, "x</SCRIPT><b>");
        assert!(html.contains("kb-error") && !html.contains("<b>"), "{html}");
        assert!(
            html.starts_with(r#"<div class="kb-block" data-plugin="raw">"#),
            "not a figure: {html}"
        );
    }

    #[test]
    fn a_command_s_output_is_the_block_and_is_kept() {
        let (dir, given) = own(
            "name = \"upper\"\nblocks = [\"upper\"]\ncommand = [\"tr\", \"a-z\", \"A-Z\"]\n",
            &[],
        );
        let config = Config {
            plugins: [("upper".to_string(), given)].into_iter().collect(),
            base: dir.path().to_path_buf(),
            ..Config::default()
        };
        let cache = dir.path().join("cache");
        let plugins =
            Plugins::load(&config, Some(Path::new("git-kb.toml")), Some(cache.clone())).unwrap();
        let plugin = plugins.block("upper").unwrap();
        assert!(
            plugins.render_block(plugin, "shout").contains(">SHOUT"),
            "made"
        );
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1, "kept");
        assert!(
            plugins.render_block(plugin, "shout").contains(">SHOUT"),
            "taken from what was kept"
        );
    }

    #[test]
    fn a_command_that_is_not_there_says_so_in_place_of_its_block() {
        let (dir, given) = own(
            "name = \"gone\"\nblocks = [\"gone\"]\ncommand = [\"no-such-program-here\"]\n",
            &[],
        );
        let plugins = load(&dir, &[("gone", given)]).unwrap();
        let html = plugins.render_block(plugins.block("gone").unwrap(), "x");
        assert!(
            html.contains("kb-error") && html.contains("not installed"),
            "{html}"
        );
    }

    #[test]
    fn an_svg_is_taken_from_where_it_begins() {
        assert_eq!(
            clean_svg("<?xml version=\"1.0\"?>\n<svg>x</svg>\n"),
            "<svg>x</svg>"
        );
        assert_eq!(clean_svg("<p>no svg</p>"), "<p>no svg</p>");
    }

    #[test]
    fn without_git_kb_toml_every_built_in_plugin_is_used() {
        let plugins = Plugins::load(&Config::default(), None, None).unwrap();
        let names: Vec<&str> = plugins.all().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, built_in_names());
    }

    #[test]
    fn tikz_diagrams_are_compiled_in_the_browser_with_the_shared_preamble() {
        let mut given = toml::Table::new();
        given.insert(
            "libraries".into(),
            toml::Value::Array(vec!["calc".into(), "fit".into()]),
        );
        let dir = tempfile::tempdir().unwrap();
        let plugins = load(&dir, &[("tikz", given)]).unwrap();
        let tikz = plugins.block("tikz").unwrap();
        assert!(tikz.figure);
        let html = plugins.render_block(tikz, "\\draw (0,0) -- (1,1);");
        assert!(
            html.contains(
                r#"<script type="text/tikz" data-tikz-libraries="calc,fit" data-add-to-preamble=""#
            ),
            "{html}"
        );
        assert!(html.contains("\\draw (0,0) -- (1,1);</script>"), "{html}");
        assert!(
            html.contains("sfcontrol"),
            "the preamble's inks are in it: {html}"
        );
        let head = plugins.head(&[&html], "/");
        assert!(
            head.contains("tikzjax.js") && head.contains("/_plugins/tikz/tikz.js"),
            "{head}"
        );
        assert!(
            plugins
                .file("tikz", "tikz.js")
                .is_some_and(|(_, js)| !js.is_empty())
        );
    }

    #[test]
    fn mermaid_diagrams_are_drawn_in_the_browser_from_their_escaped_source() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = load(&dir, &[("mermaid", toml::Table::new())]).unwrap();
        let mermaid = plugins.block("mermaid").unwrap();
        let html = plugins.render_block(mermaid, "graph TD; A-->B<C");
        assert!(
            html.contains(r#"<pre class="kb-mermaid">graph TD; A--&gt;B&lt;C</pre>"#),
            "{html}"
        );
        let head = plugins.head(&[&html], "/");
        assert!(
            head.contains("mermaid.min.js") && head.contains("/_plugins/mermaid/mermaid.css"),
            "{head}"
        );
    }

    #[test]
    fn wavedrom_diagrams_are_read_as_data_not_run() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = load(&dir, &[("wavedrom", toml::Table::new())]).unwrap();
        let wavedrom = plugins.block("wavedrom").unwrap();
        let html = plugins.render_block(wavedrom, "{signal: [{name: '<clk>', wave: 'p...'}]}");
        assert!(
            html.contains(r#"<pre class="kb-wavedrom-source">{signal: [{name: '&lt;clk&gt;', wave: 'p...'}]}</pre>"#),
            "{html}"
        );
        assert!(!html.contains("<script>"), "no source is run: {html}");
        let head = plugins.head(&[&html], "/");
        assert!(
            head.contains("json5") && head.contains("wavedrom.min.js"),
            "{head}"
        );
    }
}
