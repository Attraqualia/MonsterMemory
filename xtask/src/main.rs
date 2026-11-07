//! Repository maintenance commands.

use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

fn main() -> ExitCode {
    let command = env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    let root = match workspace_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("xtask: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = match command.as_str() {
        "manual" => build_manual(&root),
        "manual-check" => check_manual(&root),
        _ => {
            eprintln!("usage: cargo xtask <manual|manual-check>");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> Result<PathBuf, String> {
    let mut candidate = env::current_dir().map_err(|error| error.to_string())?;
    loop {
        let manifest = candidate.join("Cargo.toml");
        if fs::read_to_string(&manifest).is_ok_and(|contents| contents.contains("[workspace]")) {
            return Ok(candidate);
        }
        if !candidate.pop() {
            return Err("cannot locate the Cargo workspace root".to_owned());
        }
    }
}

fn build_manual(root: &Path) -> Result<(), String> {
    let html = render_manual(root)?;
    let output = root.join("docs/manual/html");
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    fs::write(output.join("index.html"), html).map_err(|error| error.to_string())?;
    fs::write(output.join("404.html"), not_found_page()).map_err(|error| error.to_string())?;
    println!("generated {}", output.join("index.html").display());
    Ok(())
}

fn render_manual(root: &Path) -> Result<String, String> {
    let source_path = root.join("docs/manual/src/manual.md");
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("cannot read {}: {error}", source_path.display()))?;
    let source = expand_includes(root, &source)?;
    let (body, navigation) = render_markdown(&source);
    Ok(page(&body, &navigation))
}

fn check_manual(root: &Path) -> Result<(), String> {
    let html_path = root.join("docs/manual/html/index.html");
    let html = fs::read_to_string(&html_path).map_err(|error| error.to_string())?;
    let expected = render_manual(root)?;
    if html != expected {
        return Err("generated manual differs from source; run `cargo xtask manual`".to_owned());
    }
    let required = [
        "Overview",
        "5-minute Quick Start",
        "Installation",
        "Schema Definition",
        "Primary Key",
        "Secondary / Unique Index",
        "Range / Composite Index",
        "CRUD",
        "Batch Write / Bulk Load",
        "Transactions",
        "Snapshot / MVCC Semantics",
        "Concurrency Model",
        "Error Handling",
        "Validation",
        "Persistence / Durability",
        "Schema Evolution",
        "Compaction / Memory Reclamation",
        "Performance Tuning",
        "Metrics / Tracing",
        "Testing Strategy",
        "Architecture",
        "Safety Invariants",
        "Benchmark Methodology and Results",
        "Migration Guide",
        "Troubleshooting",
        "FAQ",
        "Feature Flags",
        "MSRV / Platform Support",
        "Security / Corrupt Input Handling",
        "API Reference",
    ];
    for chapter in required {
        if !html.contains(chapter) {
            return Err(format!("manual is missing required chapter `{chapter}`"));
        }
    }
    for example in [
        "quickstart.rs",
        "indexes.rs",
        "transactions.rs",
        "snapshots.rs",
    ] {
        if !html.contains(example) {
            return Err(format!("manual no longer includes {example}"));
        }
    }
    for token in [
        "<main",
        "<nav",
        "@media",
        "prefers-color-scheme",
        "focus-visible",
        "window.print",
    ] {
        if !html.contains(token) {
            return Err(format!("manual output is missing quality marker `{token}`"));
        }
    }
    if !root.join("docs/manual/html/404.html").is_file() {
        return Err("manual 404 page is missing".to_owned());
    }
    check_local_links(&html, &html_path)?;
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(root)
        .args([
            "check",
            "-p",
            "monstermemory",
            "--examples",
            "--all-features",
        ])
        .status()
        .map_err(|error| format!("cannot run example compile check: {error}"))?;
    if !status.success() {
        return Err("manual example compile check failed".to_owned());
    }
    println!("manual-check: 30 chapters, included examples, accessibility assets: ok");
    Ok(())
}

fn check_local_links(html: &str, html_path: &Path) -> Result<(), String> {
    let mut rest = html;
    while let Some(position) = rest.find("href=\"") {
        rest = &rest[position + 6..];
        let Some(end) = rest.find('"') else {
            return Err("manual contains an unterminated href".to_owned());
        };
        let target = &rest[..end];
        rest = &rest[end + 1..];
        if target.is_empty()
            || target.starts_with('#')
            || target.starts_with("https://")
            || target.starts_with("http://")
        {
            continue;
        }
        let relative = target.split('#').next().unwrap_or_default();
        let resolved = html_path
            .parent()
            .expect("generated HTML has a parent")
            .join(relative);
        if !resolved.is_file() {
            return Err(format!("broken local manual link `{target}`"));
        }
    }
    Ok(())
}

fn expand_includes(root: &Path, source: &str) -> Result<String, String> {
    let mut output = String::new();
    for line in source.lines() {
        if let Some(include) = line
            .trim()
            .strip_prefix("{{#include ")
            .and_then(|value| value.strip_suffix("}}"))
        {
            let path = root.join(include.trim());
            let code = fs::read_to_string(&path)
                .map_err(|error| format!("cannot include {}: {error}", path.display()))?;
            output.push_str("```rust\n");
            let _ = writeln!(output, "// included from {}", include.trim());
            output.push_str(&code);
            output.push_str("\n```\n");
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    Ok(output)
}

fn render_markdown(source: &str) -> (String, String) {
    let mut body = String::new();
    let mut navigation = String::new();
    let mut code = false;
    let mut list = false;
    let mut section = false;
    for line in source.lines() {
        if let Some(language) = line.strip_prefix("```") {
            if code {
                body.push_str("</code></pre></div>");
                code = false;
            } else {
                if list {
                    body.push_str("</ul>");
                    list = false;
                }
                let _ = write!(
                    body,
                    "<div class=\"code\"><button class=\"copy\" aria-label=\"コードをコピー\">Copy</button><pre><code class=\"language-{}\">",
                    escape(language)
                );
                code = true;
            }
            continue;
        }
        if code {
            body.push_str(&escape(line));
            body.push('\n');
            continue;
        }
        if let Some(title) = line.strip_prefix("## ") {
            if list {
                body.push_str("</ul>");
                list = false;
            }
            if section {
                body.push_str("</section>");
            }
            let id = slug(title);
            let _ = write!(body, "<section><h2 id=\"{id}\">{}</h2>", inline(title));
            let _ = write!(navigation, "<a href=\"#{id}\">{}</a>", escape(title));
            section = true;
        } else if let Some(title) = line.strip_prefix("# ") {
            let _ = write!(body, "<h1>{}</h1>", inline(title));
        } else if let Some(item) = line.strip_prefix("- ") {
            if !list {
                body.push_str("<ul>");
                list = true;
            }
            let _ = write!(body, "<li>{}</li>", inline(item));
        } else if let Some(note) = line.strip_prefix("> ") {
            if list {
                body.push_str("</ul>");
                list = false;
            }
            let _ = write!(body, "<aside role=\"note\">{}</aside>", inline(note));
        } else if line.trim().is_empty() {
            if list {
                body.push_str("</ul>");
                list = false;
            }
        } else {
            if list {
                body.push_str("</ul>");
                list = false;
            }
            let _ = write!(body, "<p>{}</p>", inline(line));
        }
    }
    if list {
        body.push_str("</ul>");
    }
    // Section end tags are optional for layout but explicit markup is nicer for validators.
    if section {
        body.push_str("</section>");
    }
    (body, navigation)
}

fn inline(value: &str) -> String {
    let escaped = escape(value);
    let mut output = String::new();
    let mut rest = escaped.as_str();
    while let Some(start) = rest.find('`') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('`') {
            output.push_str("<code>");
            output.push_str(&after[..end]);
            output.push_str("</code>");
            rest = &after[end + 1..];
        } else {
            output.push('`');
            rest = after;
        }
    }
    output.push_str(rest);
    output
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn slug(title: &str) -> String {
    title
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

fn page(body: &str, navigation: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="ja"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>MonsterMemory 開発者マニュアル</title><meta name="description" content="MonsterMemory 0.1 alpha 日本語開発者マニュアル">
<style>
:root{{--bg:#f7f7f5;--panel:#fff;--text:#202124;--muted:#5f6368;--accent:#9c3b18;--line:#d8d8d4;--code:#202328}}
@media(prefers-color-scheme:dark){{:root{{--bg:#151719;--panel:#1d2023;--text:#e8eaed;--muted:#aeb4ba;--accent:#f29b6d;--line:#3b4045;--code:#101214}}}}
*{{box-sizing:border-box}}html{{scroll-behavior:smooth}}body{{margin:0;background:var(--bg);color:var(--text);font:16px/1.72 system-ui,"Yu Gothic UI",sans-serif}}
a{{color:var(--accent)}}a:focus-visible,button:focus-visible,input:focus-visible{{outline:3px solid var(--accent);outline-offset:3px}}
.layout{{display:grid;grid-template-columns:280px minmax(0,1fr)}}nav{{position:sticky;top:0;height:100vh;overflow:auto;padding:1.4rem;background:var(--panel);border-right:1px solid var(--line)}}
nav strong{{font-size:1.2rem}}nav a{{display:block;padding:.28rem 0;color:var(--muted);text-decoration:none}}nav a:hover{{color:var(--accent)}}
main{{width:min(900px,calc(100% - 3rem));margin:0 auto;padding:3rem 0 8rem}}h1{{font-size:clamp(2rem,5vw,3.6rem);line-height:1.05}}h2{{margin-top:4rem;padding-top:1rem;border-top:1px solid var(--line);font-size:1.65rem}}code{{font-family:"Cascadia Code",Consolas,monospace}}p code,li code{{background:var(--panel);padding:.12rem .3rem;border:1px solid var(--line);border-radius:4px}}
.code{{position:relative}}pre{{overflow:auto;background:var(--code);color:#f2f2ef;padding:1.3rem;border-radius:8px;line-height:1.5}}.copy{{position:absolute;right:.6rem;top:.6rem}}aside{{border-left:5px solid var(--accent);background:var(--panel);padding:.8rem 1rem;margin:1rem 0}}.tools{{display:flex;gap:.5rem;margin:1rem 0}}input{{width:100%;padding:.55rem}}
@media(max-width:800px){{.layout{{display:block}}nav{{position:relative;height:auto;border-right:0;border-bottom:1px solid var(--line)}}nav .chapters{{display:none}}main{{width:min(100% - 2rem,900px);padding-top:1.5rem}}}}
@media print{{nav,.tools,.copy{{display:none!important}}.layout{{display:block}}main{{width:100%;padding:0}}pre{{white-space:pre-wrap}}}}
</style></head><body><div class="layout"><nav aria-label="章ナビゲーション"><strong>MonsterMemory</strong><div>v0.1.0-alpha.1</div>
<div class="tools"><input id="search" type="search" placeholder="本文を検索" aria-label="本文を検索"><button onclick="window.print()">Print</button></div><div class="chapters">{navigation}</div></nav>
<main id="content">{body}</main></div><script>
document.querySelectorAll('.copy').forEach(b=>b.addEventListener('click',()=>navigator.clipboard.writeText(b.nextElementSibling.innerText).then(()=>{{b.textContent='Copied'}})));
document.getElementById('search').addEventListener('input',e=>{{let q=e.target.value.toLowerCase();document.querySelectorAll('main section').forEach(s=>s.hidden=q&&!s.innerText.toLowerCase().includes(q))}});
</script></body></html>"#
    )
}

fn not_found_page() -> &'static str {
    "<!doctype html><html lang=\"ja\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>404 - MonsterMemory</title><main><h1>404</h1><p>ページが見つかりません。</p><a href=\"index.html\">マニュアルへ戻る</a></main></html>"
}
