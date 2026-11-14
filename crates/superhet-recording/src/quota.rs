//! Disk quota planning: which finished recordings to delete (oldest first, never the ones marked
//! keep) so that the total fits the configured limit.

/// A finished recording as the planner sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Database id.
    pub id: i64,
    /// Bytes on disk.
    pub bytes: u64,
    /// Start time, Unix milliseconds (oldest goes first).
    pub started_ms: i64,
    /// Marked keep: never deleted automatically.
    pub keep: bool,
}

/// Outcome of [`plan_cleanup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Ids to delete, oldest first.
    pub delete: Vec<i64>,
    /// Total bytes after the deletions.
    pub used_after: u64,
    /// Whether the total fits `limit` afterwards.
    pub fits: bool,
}

/// Plan the deletions needed to get below `limit` bytes. `extra` is what is not in `entries`
/// (recordings still being written, the space a new one needs). Nothing is deleted while the
/// total fits; once it does not, deletion continues down to 90 % of the limit so the cleanup
/// does not run on every write.
#[must_use]
pub fn plan_cleanup(entries: &[Entry], extra: u64, limit: u64) -> Plan {
    let mut used = extra.saturating_add(entries.iter().map(|e| e.bytes).sum::<u64>());
    if used <= limit {
        return Plan {
            delete: Vec::new(),
            used_after: used,
            fits: true,
        };
    }
    let target = limit / 10 * 9;
    let mut candidates: Vec<&Entry> = entries.iter().filter(|e| !e.keep).collect();
    candidates.sort_by_key(|e| (e.started_ms, e.id));
    let mut delete = Vec::new();
    for entry in candidates {
        if used <= target {
            break;
        }
        used = used.saturating_sub(entry.bytes);
        delete.push(entry.id);
    }
    Plan {
        delete,
        used_after: used,
        fits: used <= limit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: i64, bytes: u64, started_ms: i64, keep: bool) -> Entry {
        Entry {
            id,
            bytes,
            started_ms,
            keep,
        }
    }

    #[test]
    fn nothing_is_deleted_while_it_fits() {
        let plan = plan_cleanup(&[e(1, 40, 1, false), e(2, 40, 2, false)], 10, 100);
        assert!(plan.delete.is_empty() && plan.fits);
        assert_eq!(plan.used_after, 90);
    }

    #[test]
    fn oldest_unkept_recordings_go_first_down_to_ninety_percent() {
        let entries = [
            e(3, 30, 300, false),
            e(1, 30, 100, false),
            e(2, 30, 200, true),
            e(4, 30, 400, false),
        ];
        let plan = plan_cleanup(&entries, 0, 100);
        // 120 bytes used; target 90: delete id 1 (oldest, unkept), then 3 is not needed.
        assert_eq!(plan.delete, vec![1]);
        assert_eq!(plan.used_after, 90);
        assert!(plan.fits);
    }

    #[test]
    fn kept_recordings_survive_even_when_that_means_it_does_not_fit() {
        let entries = [e(1, 80, 1, true), e(2, 50, 2, false)];
        let plan = plan_cleanup(&entries, 0, 100);
        assert_eq!(plan.delete, vec![2]);
        assert_eq!(plan.used_after, 80);
        let all_kept = plan_cleanup(&[e(1, 150, 1, true)], 0, 100);
        assert!(all_kept.delete.is_empty() && !all_kept.fits);
    }

    #[test]
    fn extra_usage_counts_and_ties_break_by_id() {
        let entries = [e(2, 50, 5, false), e(1, 50, 5, false)];
        let plan = plan_cleanup(&entries, 30, 100);
        assert_eq!(plan.delete, vec![1]);
        assert!(plan.fits);
    }
}
