//! The API for AI reviewers, under /_ai: JSON in and out, with a guide at /_ai that an agent can read to learn it.
//! Everything an AI writes here is marked as an AI's, by the agent it names, for the person whose server it is;
//! the browser's forms cannot write that mark, nor this API leave it off. Lines in the API count from 1, as people
//! and agents read them; inside, they count from 0.

use std::fmt::Write;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use serde_json::{Value, json};

use crate::diff::Diff;
use crate::render::{Figure, Renderer};

/// The guide, for an agent to read first: what there is, how to cite text, and how to review well.
pub const GUIDE: &str = r#"# Reviewing these docs, for AI agents

This server shows a folder of Markdown docs, kept in git. People read them in a browser and comment on them; you
can comment too, through this API. Everything you write is marked as written by an AI, under the agent name you
give, and shows to people with a 🤖 AI label. Be specific, be useful, and do not pretend to be a person.

All requests and answers are JSON. Lines count from 1. Send `Content-Type: application/json` with every POST.

## Read

- `GET /_ai/pages` — every page: its path, title and when it last changed.
- `GET /_ai/page?path=/arch/overview.md` — one page: its source, its numbered lines, its blocks (paragraphs,
  headings, lists, code, diagrams) with the lines each spans and its plain text, and the open threads on it.
  Add `&rev=<branch, tag or commit>` to read the page as it was there.
- `GET /_ai/threads?page=/arch/overview.md` — the threads on a page, open and resolved, with every comment.

Read the page's existing threads before you comment, and do not raise again what is already raised: reply
to that thread instead, if you have something to add.

## Comment

`POST /_ai/threads` begins a thread:

```json
{
  "agent": "Claude (claude-opus-5-5)",
  "page": "/arch/overview.md",
  "quote": "Dispatch costs about 9 core cycles per tasklet",
  "kind": "concern",
  "body": "Where does 9 come from? Name the co-simulation run that measured it."
}
```

- `agent` (required): who you are, as people should see it, such as your product and model.
- `page` (required): the page's path, as `/_ai/pages` gives it.
- `quote`: the exact words you comment on, as they read on the page or in its source, a few words or a
  whole paragraph. They are found for you. If they are not on the page, or are on it more than once, the
  answer says so, with the places that come closest or that match, so that you can quote more exactly or more
  words. Instead of a quote you can give `"lines": {"first": 12, "last": 14}`.
- `kind` (required): `concern` (something wrong, risky, contradictory or missing), `question` (something
  unclear), `suggestion` (a change you propose, with the wording if you can) or `note` (anything else).
- `body` (required): Markdown. Say what is wrong and why, and what would fix it. One point a thread.

The answer gives the thread's `id`, the lines it is on, and `url`, where a person sees it.

`POST /_ai/threads/<id>/replies` with `{"agent": "...", "body": "..."}` replies to a thread.

`POST /_ai/threads/<id>/resolve` with `{"agent": "...", "resolved": true}` resolves a thread, or reopens it with
`false`. You may resolve only threads an AI began; people's threads are theirs to resolve.

## Reviewing well

- Comment on the text itself: claims without support, numbers without a source, contradictions between
  sections or pages, terms used before they are defined, steps missing, ambiguity, and wrong or broken links.
- Quote the shortest words that make the point plain, and keep each thread to one point.
- Prefer a few comments that matter over many that do not.
- Diagrams are source in fenced blocks, such as ```tikz or ```mermaid; you can quote that source too. A diagram labelled on its fence,
  ```tikz #fig:life "Data life cycle", is a numbered figure, given in its block's `figure`; text refers to it as
  @fig:life, or @other.md#fig:life on another page, and readers see "Figure 2". Refer to figures by their number.
"#;

/// A block of a page, for an agent: the lines it spans, from 1, what it is, and its text without its markup.
#[must_use]
pub fn blocks(text: &str) -> Vec<Value> {
    let line = line_of(text);
    let renderer = Renderer::new();
    let figures = renderer.figures(text);
    renderer.block_ranges(text).into_iter().map(|r| {
        let (first, last) = (line(r.start) + 1, line(r.end.saturating_sub(1).max(r.start)) + 1);
        let block = &text[r.clone()];
        let mut value = json!({ "lines": { "first": first, "last": last }, "kind": kind(block), "text": plain(block) });
        // A labelled diagram's figure: its label, to refer to it by @label, its number and its caption.
        let fence = block.lines().next().unwrap_or("").trim_start_matches(['`', '~']);
        if let Some((id, _)) = Figure::label(fence) && let Some(f) = figures.iter().find(|f| f.id == id) {
            value["figure"] = json!({ "label": f.id, "number": f.number, "caption": f.caption });
        }
        value
    }).collect()
}

/// What a block is: heading, paragraph, list, quote, table, code, diagram or html.
fn kind(block: &str) -> &'static str {
    match Parser::new_ext(block, Renderer::new().options()).next() {
        Some(Event::Start(Tag::Heading { .. })) => "heading",
        Some(Event::Start(Tag::List(_))) => "list",
        Some(Event::Start(Tag::BlockQuote(_))) => "quote",
        Some(Event::Start(Tag::Table(_))) => "table",
        Some(Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))))
            if crate::plugins::get().is_figure(Renderer::lang(&info)) =>
        {
            "diagram"
        }
        Some(Event::Start(Tag::CodeBlock(_))) => "code",
        Some(Event::Start(Tag::HtmlBlock) | Event::Html(_)) => "html",
        _ => "paragraph",
    }
}

/// A block's words without its markup, with its breaks as spaces; code as it is.
fn plain(block: &str) -> String {
    let mut out = String::new();
    for event in Parser::new_ext(block, Renderer::new().options()) {
        match event {
            Event::Text(t) | Event::Code(t) => out.push_str(&t),
            Event::InlineMath(m) => {
                let _ = write!(out, "${m}$");
            }
            Event::DisplayMath(m) => {
                let _ = write!(out, "$${m}$$");
            }
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            Event::End(_) if !out.ends_with(' ') && !out.is_empty() => out.push(' '),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn line_of(text: &str) -> impl Fn(usize) -> usize + '_ {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    move |offset| starts.partition_point(|&s| s <= offset).saturating_sub(1)
}

/// A block's span in the page's words, and its lines.
type Span = (std::ops::Range<usize>, std::ops::Range<usize>);

/// Where quoted words are on a page: the lines they span, from 0, end excluded, and the words with those just
/// before and after them, as a snippet's comment keeps them.
pub struct Found {
    pub lines: std::ops::Range<usize>,
    pub exact: String,
    pub prefix: String,
    pub suffix: String,
}

/// Finds quoted words in a page: in its blocks' text without markup, or else in its source, so that an agent can
/// quote either. Runs of white space count as one space; case counts, unless nothing matches with it.
///
/// # Errors
///
/// When the words are not on the page, or are on it more than once: JSON saying so, with the places that come
/// closest, or that match.
#[expect(clippy::too_many_lines)]
pub fn find(text: &str, quote: &str) -> Result<Found, Value> {
    let squeeze = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let want = squeeze(quote);
    if want.is_empty() {
        return Err(json!({ "error": "the quote is empty" }));
    }
    let line = line_of(text);
    // The page as one string of its blocks' words, each block's span in it, and its lines.
    let ranges = Renderer::new().block_ranges(text);
    let mut whole = String::new();
    let mut spans: Vec<Span> = vec![];
    for r in &ranges {
        let words = squeeze(&plain(&text[r.clone()]));
        let start = whole.len();
        whole.push_str(&words);
        spans.push((
            start..whole.len(),
            line(r.start)..line(r.end.saturating_sub(1).max(r.start)) + 1,
        ));
        whole.push(' ');
    }
    let (source, offsets) = squeezed(text);
    let hits = |hay: &str, fold: bool| -> Vec<usize> {
        let (hay, want) = if fold {
            (hay.to_lowercase(), want.to_lowercase())
        } else {
            (hay.to_string(), want.clone())
        };
        hay.match_indices(&want).map(|(i, _)| i).collect()
    };
    let around = |hay: &str, at: usize| {
        let floor = |mut i: usize| {
            while !hay.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        let (a, b) = (
            floor(at.saturating_sub(32)),
            floor((at + want.len() + 32).min(hay.len())),
        );
        (
            hay[a..at].to_string(),
            hay[floor(at + want.len()).min(hay.len())..b].to_string(),
        )
    };
    for fold in [false, true] {
        let found = hits(&whole, fold);
        if found.len() == 1 {
            let at = found[0];
            let first = spans
                .iter()
                .find(|(s, _)| s.end > at)
                .map_or(0, |(_, l)| l.start);
            let last = spans
                .iter()
                .rev()
                .find(|(s, _)| s.start < at + want.len())
                .map_or(first + 1, |(_, l)| l.end);
            let (prefix, suffix) = around(&whole, at);
            return Ok(Found {
                lines: first..last,
                exact: whole[at..at + want.len()].to_string(),
                prefix,
                suffix,
            });
        }
        if found.len() > 1 {
            let places: Vec<Value> = found.iter().take(10).map(|&at| {
                let lines = spans.iter().find(|(s, _)| s.end > at).map(|(_, l)| l.clone()).unwrap_or(0..1);
                let (before, after) = around(&whole, at);
                json!({ "lines": { "first": lines.start + 1, "last": lines.end }, "context": format!("{before}[{want}]{after}") })
            }).collect();
            return Err(
                json!({ "error": format!("the quote is on the page {} times", found.len()),
                "hint": "quote more words, so that only one place matches, or give lines instead", "matches": places }),
            );
        }
        // In the source, as it is written, for quotes of markup or of a diagram's TikZ: found in the source with its
        // white space made single, and turned back into the source's own offsets, and so its lines.
        let found = hits(&source, fold);
        if found.len() == 1 {
            let at = found[0];
            let (Some(&start), Some(&end)) = (offsets.get(at), offsets.get(at + want.len() - 1))
            else {
                return Err(
                    json!({ "error": "the quote matched, but where it is could not be told; give lines instead" }),
                );
            };
            let (prefix, suffix) = around(&source, at);
            return Ok(Found {
                lines: line(start)..line(end) + 1,
                exact: source[at..at + want.len()].to_string(),
                prefix,
                suffix,
            });
        }
    }
    // A quote of the source's markup, such as **bold**, may be meant as the words it shows: those, without it.
    let bare: String = want
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`'))
        .collect();
    if bare != want && !bare.trim().is_empty() {
        return find(text, &bare);
    }
    let mut near: Vec<(f64, &Span)> = spans
        .iter()
        .map(|span| (Diff::similarity(&want, &whole[span.0.clone()]), span))
        .collect();
    near.sort_by(|a, b| b.0.total_cmp(&a.0));
    let near: Vec<Value> = near.iter().take(3).filter(|(likeness, _)| *likeness > 0.0).map(|(likeness, (s, l))| json!({
        "lines": { "first": l.start + 1, "last": l.end }, "likeness": (likeness * 100.0).round() / 100.0, "text": &whole[s.clone()] })).collect();
    Err(
        json!({ "error": "the quote is not on the page", "hint": "quote the words exactly as they read; these come closest", "closest": near }),
    )
}

/// A text with its runs of white space made one space and its ends trimmed, and, for each byte of that, the offset
/// in the text it came from.
fn squeezed(text: &str) -> (String, Vec<usize>) {
    let (mut out, mut offsets, mut space) = (String::new(), vec![], true);
    for (i, c) in text.char_indices() {
        let white = c.is_whitespace();
        if white && space {
            continue;
        }
        let c = if white { ' ' } else { c };
        out.push(c);
        offsets.extend(std::iter::repeat_n(i, c.len_utf8()));
        space = white;
    }
    if out.ends_with(' ') {
        out.pop();
        offsets.pop();
    }
    (out, offsets)
}
