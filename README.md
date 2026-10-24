# git-kb

**A knowledge base of Markdown in git.**

|  |  |  |
| --- | --- | --- |
| [![fmt](https://github.com/oddcoder/git-kb/actions/workflows/fmt.yml/badge.svg?branch=main)](https://github.com/oddcoder/git-kb/actions/workflows/fmt.yml) | [![build](https://github.com/oddcoder/git-kb/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/oddcoder/git-kb/actions/workflows/build.yml) | [![test](https://github.com/oddcoder/git-kb/actions/workflows/test.yml/badge.svg?branch=main)](https://github.com/oddcoder/git-kb/actions/workflows/test.yml) |
| [![clippy](https://github.com/oddcoder/git-kb/actions/workflows/clippy.yml/badge.svg?branch=main)](https://github.com/oddcoder/git-kb/actions/workflows/clippy.yml) | [![deny](https://github.com/oddcoder/git-kb/actions/workflows/deny.yml/badge.svg?branch=main)](https://github.com/oddcoder/git-kb/actions/workflows/deny.yml) | [![advisories](https://github.com/oddcoder/git-kb/actions/workflows/advisories.yml/badge.svg)](https://github.com/oddcoder/git-kb/actions/workflows/advisories.yml) |

![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)

git-kb shows a folder of Markdown in a git repository as a site to read. Pages link to each other as they are written, folders are listed or shown by their `index.md` or `README.md`, and every page has a sidebar of its headings and the site's pages. It serves the site while you write, and open pages update as you save, where you are on them. It also builds the site as a static folder of HTML that you can open from disk or put on any host, such as GitHub Pages.

## Installing

You need:

- **Rust**, through [rustup](https://rustup.rs). The repository pins the toolchain it builds with, and rustup installs it the first time you build.
- **Node.js** with npm, from [nodejs.org](https://nodejs.org) or your package manager. The build compiles the pages' TypeScript with the TypeScript and esbuild versions in `web/package.json`. It installs them under `target/web` the first time.

Then install from GitHub:

```sh
cargo install --locked --git https://github.com/oddcoder/git-kb
```

Or install from a clone:

```sh
git clone https://github.com/oddcoder/git-kb
cd git-kb
cargo install --locked --path .
```

Either way, `cargo install` puts `git-kb` in `~/.cargo/bin`, which should be on your `PATH`. Because the binary is named `git-kb`, git also runs it as `git kb`.

## Using

Run it in the repository, or in any folder of Markdown:

```sh
git kb serve           # serves the site at http://127.0.0.1:8000/docs/
git kb build           # writes the site into target/git-kb/docs/, and each branch and tag into target/git-kb/ref/
git kb serve docs      # the same, starting from the folder docs
```

Each command takes a folder, `.` by default. git-kb looks for a `git-kb.toml` file in that folder and in each folder above it, up to the top of the repository. Without one, it shows the folder itself with every setting at its default. `git kb -h` lists the commands. `git kb --help` doesn't work, because git turns it into a request for a manual page.

In a git repository, `git kb serve` also shows the site at any commit, under `/ref/`: `/ref/main/`, `/ref/v1.0/arch/` or `/ref/3f2a1c9/`. The part after `/ref/` is anything `git rev-parse` takes, with a `/` in a branch name written as `%2F`. Each page says when it last changed, by whom, and in which commit. So does each paragraph, in the margin, when you point at it; clicked, it shows how the paragraph changed then. Add `?history` to a page's address to list the commits that changed it, and `/ref/` lists the branches, tags and commits. Add `?diff=<rev>` to compare a page with itself at another commit, its changed blocks and words marked: `?diff=HEAD` shows what has changed since the last commit, and each page links what its last commit changed. `git kb build` writes the same, apart from the history pages and comparisons with any commit, which need the server; each page's comparisons for the commits that changed it, and for its uncommitted changes, are written beside it, as `arch/overview~3f2a1c9.html`; each branch and tag goes into `ref/<name>/`, with their list in `ref/index.html`.

The sidebar shows the page's links: a graph of the pages around it, and each page it links to or that links to it, with the sections the links sit in; a page linked to that is not there is marked missing. Its ⤢ opens the graph of every page. The search box in the bar finds the pages that hold every word asked for, best first, each linked to its section that holds them most; served, at the commit shown, and built, in the browser, from an index beside each site. Drag the sidebar's edge to size it, or to close it; drag the text's right edge to set its width, and its left edge to move it. Double-click an edge for its default. The layout is kept in your browser, as the look is. Each page has a Page / Source switch at its top right, to read its Markdown instead, highlighted. When served, `?raw` after a page's address gives its Markdown as it is.

In a git repository, pages take review comments, kept in git but off every branch. Served, point at a paragraph and press its **+** to begin a thread on it; each thread has a box to reply in and a button to resolve or reopen it. The threads sit in a column beside the page, in its order, and follow their text as the page is edited, moved or renamed; a thread whose text has gone is listed under the rest. A comment is Markdown, with any HTML in it shown as text; `[[Page title]]` or `[[arch/overview.md]]` in it links to that page. `/_comments` lists every thread of the site. Each person's comments are kept under their own ref, `refs/git-kb/comments/<their email>`, written as git's `user.name` and `user.email`, so the working tree, the index and every branch are left as they are. **Sync** in the column, which the server also does every few minutes, fetches everyone's comments from each remote and pushes yours, with git's own credentials. Pushes never collide, as each ref has one writer. At a commit, and built, the comments are only shown.

A link to a page that is not there is marked, dashed, saying so when pointed at, served and built alike.

What `.gitignore` files ignore is never shown or built. To leave out more, such as drafts, list it in a `.kbignore` file, which works like `.gitignore`.

## Settings

Every setting in `git-kb.toml` is optional. Paths are relative to the file:

```toml
root = "docs"                     # the folder of Markdown; the file's own folder by default
title = "pentasil"                # the site's name, in its bar and its pages' titles; the root folder's by default
url = "https://foobar.xyz/kb/"    # where the built site's top lives, for its pages' canonical addresses

[serve]
host = "127.0.0.1"                # the address to listen on; only this machine by default
port = 8000
sync-every = 5                    # minutes between syncs of the comments with every remote; 0 for never

[build]
out = "target/git-kb"             # where the site is written; git kb build only empties a folder it wrote itself

[comments]
namespace = "refs/git-kb/comments"  # each person's comments are kept under <namespace>/<their email>

[plugins.graphviz]                # a plugin by name, built in or, by path, the repository's own; its options go in its table
path = "tools/kb-graphviz"

[look]                            # how pages look until a reader chooses otherwise in the Aa menu
scheme = "auto"                   # light, dark, or auto, as the reader's system is
palette = "github"                # github, paper, solarized, nord or gruvbox
size = 16                         # the text size in pixels: 14, 15, 16, 18 or 20
font = "sans"                     # the text font: sans, serif or mono
code = "system"                   # the code font: system, menlo or courier
```

## Plugins

Plugins render what Markdown alone does not: fenced blocks such as ```` ```tikz ````, and math such as `$x^2$`. Only the plugins `git-kb.toml` names are used; without a `git-kb.toml`, every built-in one. A page loads a plugin's scripts only if it uses that plugin. `git kb plugins` lists them.

| Plugin | Renders | How |
|---|---|---|
| `tikz` | ```` ```tikz ```` | TikZJax compiles the diagram in the browser, with a preamble of shared styles, and draws it in the theme's colors |
| `mermaid` | ```` ```mermaid ```` | Mermaid draws the diagram in the browser, in the page's scheme |
| `wavedrom` | ```` ```wavedrom ```` | WaveDrom draws timing diagrams, register bitfields and logic circuits in the browser, in the theme's colors |
| `mathjax` | `$...$`, `$$...$$` | MathJax sets the math in the browser |

A diagram plugin's blocks are *figures*:
- Labelled on their fence, as in ```` ```mermaid #fig:flow "Data flow" ````, they are numbered in page order, and their number and caption show in the bar under them.
- Text refers to them as `@fig:flow`, or `@other.md#fig:flow` for one on another page. Readers see "Figure 2", with the caption when pointed at; a reference to no such figure is marked.
- Each shows its source under that bar, and can be clicked to zoom.

### Writing one

A plugin is a folder with a `plugin.toml`, named in `git-kb.toml` by its path. It renders in one of two ways:
- **In the browser:** each block becomes the plugin's `stage` HTML, which its scripts then draw.
- **When served and built:** git kb runs a `command` with the block's source on its input. The SVG or HTML on its output becomes the block. Outputs are cached by their source, so each one is made only once.

```toml
name = "graphviz"
description = "Graphviz graphs, laid out by the dot command"
blocks = ["dot"]            # the fence languages it renders
math = false                # whether it renders $...$ and $$...$$; only one plugin may
figure = true               # whether its blocks are figures: numbered, captioned, zoomable
highlight = "text"          # how its source is highlighted, under the bar

# When served and built: the source on the command's input, its SVG output the block.
command = ["dot", "-Tsvg", "-Gbgcolor=transparent", "-Nfontname={font}"]

# Or, instead of command, in the browser: the HTML each block becomes, and the scripts that draw it.
# stage = '<pre class="graph">{source.html}</pre>'
# head = ['<script src="https://cdn.example.org/viz.js" defer></script>']
# scripts = ["graphviz.js"]  # its own files, served from {assets}
# styles = ["graphviz.css"]

[options]                   # each option and its default; git-kb.toml may set any of them
font = "Helvetica"
[files]                     # options that name a file, whose content is used
# preamble = "preamble.tex"
```

These placeholders are filled in `stage`, `inline`, `display`, `head` and `command`:
- `{name}`: an option's value.
- `{source.html}`: the block's source, escaped for HTML.
- `{source.raw}`: the source as it is, for inside a `<script>`.
- `{assets}`: where the plugin's own files are served.

A plugin's scripts must follow these rules:
- They are plain scripts, not modules, so that a built site works from disk.
- They draw the blocks in `[data-plugin="name"]`.
- They draw again on the `kb-refresh` event, which fires after a live update swaps in a fresh copy of the page. They may also redraw when the reader changes the look: the `data-*` attributes of `<html>`, which a `MutationObserver` can watch.
- They mark each block they have drawn with `data-rendered`, so that a live update keeps the drawing while its source is the same.

## AI reviewers

Served, `/_ai` is a JSON API for AI agents to review the docs: a guide to it at `/_ai`, and `/llms.txt` naming it. An agent reads the pages, with their numbered lines and the threads on them, and begins threads on exact words it quotes, replies and resolves. Each comment it writes is marked as an AI's, by the agent it names and for the person whose server it is, and shows with a 🤖 AI badge; a thread it begins says what it raises: a concern, a question, a suggestion or a note. An AI resolves only threads an AI began.

## Developing

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

CI runs these, plus `cargo deny`. The browser code is TypeScript in `web/src`. `cargo build` type-checks it and compiles it, through `build.rs`, so a type error fails the build.

## License

Apache-2.0; see [LICENSE](LICENSE).
