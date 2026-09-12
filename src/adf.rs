//! ADF JSON -> plain text flatten (recursive).

use serde_json::Value;

/// AIDEV-NOTE: MVP flatten — paragraphs -> `\n\n`, hardBreak -> `\n`, headings -> line,
/// list items -> `- `. Unknown node types recurse into `content` and drop styling/marks;
/// unknown nodes without content are dropped silently. Upgrade path (plan Risks): swap for a
/// rich ADF->markdown converter (tables, panels, code blocks as blocks) if descriptions get
/// heavier; until then rich blocks degrade to plain text on purpose.
pub fn flatten(adf: &Value) -> String {
    let mut out = String::new();
    walk(adf, &mut out);
    let joined: Vec<&str> = out.lines().map(str::trim_end).collect();
    let mut result = joined.join("\n");
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    result.trim().to_string()
}

/// ponytail: leaf vs block handled by one recursive walk; per-node types if exotic nodes ever matter.
fn walk(node: &Value, out: &mut String) {
    walk_in(node, out, false)
}

fn walk_in(node: &Value, out: &mut String, in_item: bool) {
    let Some(obj) = node.as_object() else {
        if let Some(s) = node.as_str() {
            out.push_str(s); // raw string input passthrough
        }
        return;
    };
    let typ = obj.get("type").and_then(Value::as_str).unwrap_or("");
    if typ == "hardBreak" {
        out.push('\n');
        return;
    }
    if typ == "text" {
        if let Some(s) = obj.get("text").and_then(Value::as_str) {
            out.push_str(s);
        }
        return;
    }
    // block-level node: start on fresh line; list items get "- "
    if typ == "listItem" {
        out.push_str("- ");
    } else if matches!(
        typ,
        "paragraph" | "heading" | "bulletList" | "orderedList" | "codeBlock"
    ) && !(in_item && typ == "paragraph")
        && !out.is_empty()
        && !out.ends_with("\n\n")
    {
        out.push('\n');
    }
    // AIDEV-NOTE: unknown types fall through and just recurse into content (or drop if none)
    if let Some(content) = obj.get("content").and_then(Value::as_array) {
        for child in content {
            walk_in(child, out, in_item || typ == "listItem");
        }
    }
    match typ {
        "paragraph" | "heading" => out.push('\n'),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::flatten;
    use serde_json::json;

    #[test]
    fn paragraph() {
        let adf = json!({
            "type": "doc",
            "content": [{"type": "paragraph", "content": [{"type": "text", "text": "hello"}]}]
        });
        assert_eq!(flatten(&adf), "hello");
    }

    #[test]
    fn multiple_paragraphs() {
        let adf = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "one"}]},
                {"type": "paragraph", "content": [{"type": "text", "text": "two"}]}
            ]
        });
        assert_eq!(flatten(&adf), "one\n\ntwo");
    }

    #[test]
    fn heading() {
        let adf = json!({
            "type": "doc",
            "content": [{"type": "heading", "content": [{"type": "text", "text": "Title"}]}]
        });
        assert_eq!(flatten(&adf), "Title");
    }

    #[test]
    fn hard_break() {
        let adf = json!({
            "type": "paragraph",
            "content": [
                {"type": "text", "text": "line1"},
                {"type": "hardBreak"},
                {"type": "text", "text": "line2"}
            ]
        });
        assert_eq!(flatten(&adf), "line1\nline2");
    }

    #[test]
    fn bullet_list() {
        let adf = json!({
            "type": "bulletList",
            "content": [
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "a"}]}]},
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "b"}]}]}
            ]
        });
        assert_eq!(flatten(&adf), "- a\n- b");
    }

    #[test]
    fn marks_dropped_text_kept() {
        let adf = json!({
            "type": "paragraph",
            "content": [
                {"type": "text", "text": "bold", "marks": [{"type": "strong"}]},
                {"type": "text", "text": " and ", "marks": [{"type": "em"}]},
                {"type": "text", "text": "link", "marks": [{"type": "link", "attrs": {"href": "https://x"}}]}
            ]
        });
        assert_eq!(flatten(&adf), "bold and link");
    }

    #[test]
    fn unknown_node_with_content_recursed() {
        let adf = json!({
            "type": "fancyPanel",
            "content": [{"type": "paragraph", "content": [{"type": "text", "text": "inside"}]}]
        });
        assert_eq!(flatten(&adf), "inside");
    }

    #[test]
    fn unknown_node_without_content_dropped() {
        let adf = json!({"type": "emojiShortName", "attrs": {"shortName": ":smile:"}});
        assert_eq!(flatten(&adf), "");
    }

    #[test]
    fn null_and_missing_are_empty() {
        assert_eq!(flatten(&serde_json::Value::Null), "");
        assert_eq!(flatten(&json!("just a string")), "just a string");
    }

    #[test]
    fn code_block_no_panic() {
        // Out of scope richness: pin current graceful behavior (text extracted inline).
        let adf = json!({
            "type": "codeBlock",
            "content": [{"type": "text", "text": "let x = 1;"}]
        });
        assert_eq!(flatten(&adf), "let x = 1;");
    }

    #[test]
    fn collapses_excess_newlines() {
        let adf = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": []},
                {"type": "paragraph", "content": []},
                {"type": "paragraph", "content": [{"type": "text", "text": "x"}]}
            ]
        });
        assert_eq!(flatten(&adf), "x");
    }
}
