//! What the fakes' content scripts say, shared by the fake agent and the fake ACP agent:
//! markdown, edits, file reads and command output that look like a real agent's, so the desktop
//! app's transcript can be tried and measured without spending quota.

/// A thought before a scripted answer, in the shape Codex and Cursor send: a bold heading, then
/// the reasoning.
pub const THOUGHT: &str = "**Planning the answer**\n\nThe user wants to see what the \
transcript can show. I'll cover headings, lists, a table and code in a few languages, \
and keep each part short.";

/// `/markdown`: headings, emphasis, lists, a table, a quote and code in several languages.
pub const MARKDOWN: &str = r#"# Markdown in the transcript

Answers are **markdown**, with *emphasis*, `inline code` and [links](https://github.com/traista-eugen/burrow).

## Lists

- Bulleted items
  - nested one level
  - and another
- Back out again

1. Numbered steps
2. Keep their order
3. And their numbers

- [x] A task that's done
- [ ] A task that isn't

## A table

| Agent | Protocol | Streams text |
|---|---|:---:|
| Claude Code | ACP | yes |
| Codex | ACP | yes |
| Cursor | ACP | yes |
| Muse | MSP | yes |

> Quotes stand out from the text around them.

## Code

```rust
/// Adds two numbers, saturating at the bounds.
pub fn add(a: i32, b: i32) -> i32 {
    a.saturating_add(b)
}
```

```python
def fib(n: int) -> int:
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a
```

```typescript
export async function fetchJson<T>(url: string): Promise<T> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return (await response.json()) as T;
}
```

```json
{ "type": "agentMessageChunk", "turnId": "trn_1", "text": "Hello" }
```

```powershell
Get-ChildItem -Recurse -Filter *.rs | Measure-Object -Property Length -Sum
```

That's everything: headings, lists, a table, a quote and code."#;

/// `/long`: a long answer, about 1,800 words, with headings, lists and code between paragraphs.
pub fn long_answer() -> String {
    let mut text = String::from(
        "# A long answer\n\nThis answer is long on purpose, so the transcript has to keep up with a message that grows for several seconds.\n\n",
    );
    for section in 1..=8 {
        text.push_str(&format!("## Part {section}\n\n"));
        for paragraph in 1..=3 {
            text.push_str(&format!(
                "Paragraph {paragraph} of part {section}. A streamed answer arrives in pieces of \
                 about a hundred milliseconds, and each piece is appended to the message that is \
                 already on screen. Only that message is parsed again, and only from where it \
                 grew: the rest of the transcript stays as it is. This sentence is here to make \
                 the paragraph wrap over a few lines, the way real answers do when they explain \
                 something at length, with `inline code` and **bold** words along the way.\n\n"
            ));
        }
        text.push_str(&format!(
            "- Point one of part {section}\n- Point two, a little longer than the first\n- Point three\n\n"
        ));
        if section % 2 == 0 {
            text.push_str(&format!(
                "```rust\nfn part_{section}() -> usize {{\n    (1..={section}).map(|n| n * n).sum()\n}}\n```\n\n"
            ));
        }
    }
    text.push_str("That's the end of the long answer.");
    text
}

/// A file a scripted edit changes.
pub struct Edit {
    /// The file, relative to the session's folder.
    pub path: &'static str,
    /// The 1-based line the old text starts at.
    pub line: u32,
    /// The text replaced.
    pub old: &'static str,
    /// Its replacement.
    pub new: &'static str,
    /// The same change as a unified diff, as Muse prints it after an edit.
    pub unified: &'static str,
}

/// `/diff`: an edit that changes a function and adds another.
pub const LIB_EDIT: Edit = Edit {
    path: "src/lib.rs",
    line: 12,
    old: "/// Adds two numbers.\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    new: "/// Adds two numbers, saturating at the bounds.\npub fn add(a: i32, b: i32) -> i32 {\n    a.saturating_add(b)\n}\n\n/// Subtracts `b` from `a`, saturating at the bounds.\npub fn sub(a: i32, b: i32) -> i32 {\n    a.saturating_sub(b)\n}\n",
    unified: "",
};

/// `/diff`: a second edit, reported the way Muse reports one: the replaced fragment as a diff,
/// and a unified diff in the tool's output.
pub const CONFIG_EDIT: Edit = Edit {
    path: "src/config.rs",
    line: 40,
    old: "    timeout_ms: 5_000,\n    retries: 1,\n",
    new: "    timeout_ms: 10_000,\n    retries: 3,\n    backoff_ms: 250,\n",
    unified: "Edited src/config.rs\n--- a/src/config.rs\n+++ b/src/config.rs\n@@ -37,8 +37,9 @@ impl Default for Config {\n     fn default() -> Self {\n         Self {\n             name: \"burrow\".into(),\n-            timeout_ms: 5_000,\n-            retries: 1,\n+            timeout_ms: 10_000,\n+            retries: 3,\n+            backoff_ms: 250,\n         }\n     }\n }\n",
};

/// `/diff`: a new file.
pub const NEW_FILE: (&str, &str) = (
    "NOTES.md",
    "# Notes\n\n- `add` and `sub` saturate instead of overflowing.\n- The config retries three times, 250 ms apart.\n",
);

/// `/tools`: what the reads find, by title, path and content.
pub const READS: &[(&str, &str, &str)] = &[
    (
        "Read README.md",
        "README.md",
        "# Calculator\n\nA tiny library for adding and subtracting numbers.\n",
    ),
    (
        "Read src/lib.rs",
        "src/lib.rs",
        "//! The calculator.\n\nmod config;\n\n/// Adds two numbers.\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    ),
    (
        "Read tests/add.rs",
        "tests/add.rs",
        "#[test]\nfn adds() {\n    assert_eq!(calc::add(2, 2), 4);\n}\n",
    ),
];

/// `/tools`: what the searches find, by title and result.
pub const SEARCHES: &[(&str, &str)] = &[
    (
        "grep \"fn add\"",
        "src/lib.rs:6:pub fn add(a: i32, b: i32) -> i32 {",
    ),
    ("Find `**/*.rs`", "src/lib.rs\nsrc/config.rs\ntests/add.rs"),
];

/// `/tools`: the command run after the reads, and its output.
pub const TEST_COMMAND: &str = "cargo test";
/// `/tools`: what `cargo test` printed.
pub const TEST_OUTPUT: &str = "   Compiling calc v0.1.0 (D:\\scratch\\calc)\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.84s\n     Running unittests src\\lib.rs (target\\debug\\deps\\calc-5f0c2e9d2b6c1a7e.exe)\n\nrunning 3 tests\ntest tests::adds ... ok\ntest tests::adds_negatives ... ok\ntest tests::saturates ... ok\n\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n     Running tests\\add.rs (target\\debug\\deps\\add-0b8f1c4e9a2d7e31.exe)\n\nrunning 1 test\ntest adds ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";

/// The text of a file as a fenced code block, the way Claude Code shows what it read.
pub fn fenced(path: &str, text: &str) -> String {
    let lang = match path.rsplit('.').next() {
        Some("rs") => "rust",
        Some("md") => "markdown",
        _ => "",
    };
    format!("```{lang}\n{text}```")
}
