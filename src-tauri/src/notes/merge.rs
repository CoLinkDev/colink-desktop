use similar::{ChangeTag, DiffTag, TextDiff};

/// Result of a three-way merge of one scalar field.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FieldMerge {
    /// Both sides agree, or only one side changed.
    Resolved(String),
    /// Both sides changed the field to different values.
    Conflict { local: String, cloud: String },
}

impl FieldMerge {
    pub(crate) fn is_resolved(&self) -> bool {
        matches!(self, FieldMerge::Resolved(_))
    }
}

/// Three-way merge of a single scalar string field: an unchanged side loses
/// against a changed side; two different changes conflict.
pub(crate) fn merge_field(ancestor: &str, local: &str, cloud: &str) -> FieldMerge {
    if local == cloud {
        return FieldMerge::Resolved(local.to_string());
    }
    if local == ancestor {
        return FieldMerge::Resolved(cloud.to_string());
    }
    if cloud == ancestor {
        return FieldMerge::Resolved(local.to_string());
    }

    FieldMerge::Conflict {
        local: local.to_string(),
        cloud: cloud.to_string(),
    }
}

/// Three-way merge of the tag or attachment id sets: additions from either
/// side are kept, removals are honored unless the other side re-added the
/// same entry. Set merges are always resolvable.
pub(crate) fn merge_set(
    ancestor: &[String],
    local: &[String],
    cloud: &[String],
) -> Vec<String> {
    let mut resolved: Vec<String> = Vec::new();

    for id in ancestor.iter().chain(local.iter()).chain(cloud.iter()) {
        if resolved.contains(id) {
            continue;
        }
        let in_ancestor = ancestor.contains(id);
        let in_local = local.contains(id);
        let in_cloud = cloud.contains(id);

        let keep = if in_local && in_cloud {
            true
        } else if in_local {
            // Cloud removed it, or local added it.
            !in_ancestor
        } else if in_cloud {
            // Local removed it, or cloud added it.
            !in_ancestor
        } else {
            false
        };

        if keep {
            resolved.push(id.clone());
        }
    }

    resolved.sort();
    resolved
}

/// Line-based three-way merge of Markdown text. Returns `None` when the two
/// sides changed overlapping regions; the caller must then keep both
/// versions and let the user decide.
pub(crate) fn merge_markdown(ancestor: &str, local: &str, cloud: &str) -> Option<String> {
    if local == cloud {
        return Some(local.to_string());
    }
    if local == ancestor {
        return Some(cloud.to_string());
    }
    if cloud == ancestor {
        return Some(local.to_string());
    }

    let ancestor_lines: Vec<&str> = ancestor.lines().collect();
    let local_hunks = collect_hunks(ancestor, local);
    let cloud_hunks = collect_hunks(ancestor, cloud);

    let mut output: Vec<String> = Vec::new();
    let mut position = 0usize;
    let mut local_index = 0usize;
    let mut cloud_index = 0usize;

    loop {
        if local_index >= local_hunks.len() && cloud_index >= cloud_hunks.len() {
            break;
        }

        // Identical edits on both sides collapse into one.
        if local_index < local_hunks.len()
            && cloud_index < cloud_hunks.len()
            && local_hunks[local_index] == cloud_hunks[cloud_index]
        {
            let (start, end, replacement) = &local_hunks[local_index];
            output.extend(apply_hunk(&ancestor_lines, &mut position, *start, *end, replacement));
            position = *end;
            local_index += 1;
            cloud_index += 1;
            continue;
        }

        if local_index < local_hunks.len()
            && cloud_index < cloud_hunks.len()
            && hunks_overlap(&local_hunks[local_index], &cloud_hunks[cloud_index])
        {
            return None;
        }

        let take_local = if local_index >= local_hunks.len() {
            false
        } else if cloud_index >= cloud_hunks.len() {
            true
        } else {
            local_hunks[local_index].0 <= cloud_hunks[cloud_index].0
        };

        let (start, end, replacement) = if take_local {
            let hunk = &local_hunks[local_index];
            if cloud_index < cloud_hunks.len() && cloud_hunks[cloud_index].0 < hunk.1 {
                return None;
            }
            hunk
        } else {
            let hunk = &cloud_hunks[cloud_index];
            if local_index < local_hunks.len() && local_hunks[local_index].0 < hunk.1 {
                return None;
            }
            hunk
        };

        output.extend(apply_hunk(
            &ancestor_lines,
            &mut position,
            *start,
            *end,
            replacement,
        ));
        position = *end;

        if take_local {
            local_index += 1;
        } else {
            cloud_index += 1;
        }
    }

    while position < ancestor_lines.len() {
        output.push(ancestor_lines[position].to_string());
        position += 1;
    }

    let mut merged = output.join("\n");
    let ends_with_newline = local.ends_with('\n')
        || cloud.ends_with('\n')
        || (local_hunks.is_empty() && ancestor.ends_with('\n'));
    if ends_with_newline && !merged.is_empty() && !merged.ends_with('\n') {
        merged.push('\n');
    }

    Some(merged)
}

type Hunk = (usize, usize, Vec<String>);

fn hunks_overlap(left: &Hunk, right: &Hunk) -> bool {
    if left.0 == left.1 && right.0 == right.1 {
        return left.0 == right.0;
    }
    if left.0 == left.1 {
        return left.0 >= right.0 && left.0 < right.1;
    }
    if right.0 == right.1 {
        return right.0 >= left.0 && right.0 < left.1;
    }
    left.0 < right.1 && right.0 < left.1
}

fn apply_hunk(
    ancestor_lines: &[&str],
    position: &mut usize,
    start: usize,
    end: usize,
    replacement: &[String],
) -> Vec<String> {
    let mut output = Vec::new();
    while *position < start && *position < ancestor_lines.len() {
        output.push(ancestor_lines[*position].to_string());
        *position += 1;
    }
    output.extend(replacement.iter().cloned());
    *position = end.max(*position);
    output
}

fn collect_hunks(ancestor: &str, current: &str) -> Vec<Hunk> {
    let diff = TextDiff::configure().diff_lines(ancestor, current);
    let mut hunks = Vec::new();

    for op in diff.ops() {
        if op.tag() == DiffTag::Equal {
            continue;
        }
        let old_range = op.old_range();
        let replacement = diff
            .iter_changes(op)
            .filter(|change| change.tag() == ChangeTag::Insert)
            .map(|change| strip_line_terminator(change.value()).to_string())
            .collect::<Vec<_>>();
        hunks.push((old_range.start, old_range.end, replacement));
    }

    hunks.sort_by_key(|hunk| (hunk.0, hunk.1));
    hunks
}

fn strip_line_terminator(value: &str) -> &str {
    let value = value.strip_suffix('\n').unwrap_or(value);
    value.strip_suffix('\r').unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_disjoint_edits() {
        let ancestor = "line1\nline2\nline3\nline4";
        let local = "local1\nline2\nline3\nline4";
        let cloud = "line1\nline2\nline3\ncloud4";

        let merged = merge_markdown(ancestor, local, cloud).unwrap();
        assert_eq!(merged, "local1\nline2\nline3\ncloud4");
    }

    #[test]
    fn identical_edits_collapse() {
        let merged = merge_markdown("a\nb", "a\nb!", "a\nb!").unwrap();
        assert_eq!(merged, "a\nb!");
    }

    #[test]
    fn overlapping_edits_conflict() {
        let ancestor = "a\nb\nc";
        let local = "a\nlocal\nc";
        let cloud = "a\ncloud\nc";
        assert!(merge_markdown(ancestor, local, cloud).is_none());
    }

    #[test]
    fn different_insertions_at_the_same_position_conflict() {
        assert!(merge_markdown("a", "x\na", "y\na").is_none());
    }

    #[test]
    fn one_sided_change_wins() {
        assert_eq!(merge_markdown("a", "a", "b").unwrap(), "b");
        assert_eq!(merge_markdown("a", "b", "a").unwrap(), "b");
    }

    #[test]
    fn set_merge_union_and_removal() {
        let ancestor = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let local = vec!["a".to_string(), "b".to_string(), "d".to_string()];
        let cloud = vec!["a".to_string(), "c".to_string()];

        let merged = merge_set(&ancestor, &local, &cloud);
        // local removed c (cloud keeps it -> removal wins), local added d,
        // cloud removed b (local keeps it -> removal wins).
        assert_eq!(merged, vec!["a".to_string(), "d".to_string()]);
    }

    #[test]
    fn set_merge_readdition_wins() {
        let ancestor = vec!["a".to_string()];
        let local = vec!["a".to_string(), "x".to_string()];
        let cloud = vec![];
        let merged = merge_set(&ancestor, &local, &cloud);
        // The cloud removed "a" while local only kept it: removal wins.
        assert_eq!(merged, vec!["x".to_string()]);

        // Additions from either side are kept.
        let ancestor = vec!["b".to_string()];
        let local = vec!["a".to_string(), "b".to_string()];
        let cloud = vec!["b".to_string()];
        assert_eq!(merge_set(&ancestor, &local, &cloud), vec![
            "a".to_string(),
            "b".to_string()
        ]);
    }
}
