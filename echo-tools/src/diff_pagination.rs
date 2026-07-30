/// Split a unified diff into independently readable file/hunk blocks.
pub(crate) fn split_unified_diff(diff: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut file_header = Vec::new();
    let mut current_hunk = Vec::new();
    let mut file_had_hunk = false;

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            flush_chunk(&mut chunks, &file_header, &mut current_hunk);
            if !file_header.is_empty() && !file_had_hunk {
                chunks.push(file_header.join("\n"));
            }
            file_header.clear();
            file_header.push(line.to_string());
            file_had_hunk = false;
        } else if line.starts_with("@@ ") {
            flush_chunk(&mut chunks, &file_header, &mut current_hunk);
            current_hunk.push(line.to_string());
            file_had_hunk = true;
        } else if current_hunk.is_empty() {
            file_header.push(line.to_string());
        } else {
            current_hunk.push(line.to_string());
        }
    }
    flush_chunk(&mut chunks, &file_header, &mut current_hunk);
    if !file_header.is_empty() && !file_had_hunk {
        chunks.push(file_header.join("\n"));
    }
    chunks
}

fn flush_chunk(chunks: &mut Vec<String>, file_header: &[String], current_hunk: &mut Vec<String>) {
    if current_hunk.is_empty() {
        return;
    }
    let mut lines = file_header.to_vec();
    lines.append(current_hunk);
    chunks.push(lines.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_headers_for_each_hunk_and_keeps_binary_files() {
        let diff = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n@@ -9 +9 @@\n-x\n+y\ndiff --git a/image b/image\nBinary files differ";
        let chunks = split_unified_diff(diff);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.first().is_some_and(|chunk| chunk.contains("@@ -1")));
        assert!(chunks.get(1).is_some_and(|chunk| {
            chunk.contains("diff --git a/a b/a") && chunk.contains("@@ -9")
        }));
        assert!(
            chunks
                .get(2)
                .is_some_and(|chunk| chunk.contains("Binary files differ"))
        );
    }

    #[test]
    fn splits_header_only_unified_diff_hunks() {
        let diff = "--- a.txt\n+++ b.txt\n@@ -1 +1 @@\n-old\n+new\n@@ -5 +5 @@\n-a\n+b";
        let chunks = split_unified_diff(diff);
        assert_eq!(chunks.len(), 2);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.starts_with("--- a.txt\n+++ b.txt"))
        );
    }
}
