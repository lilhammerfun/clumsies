//! Coordination for background project-list reads.

#[derive(Default)]
pub(crate) struct ProjectRefresh {
    generation: u64,
    in_flight: bool,
}

impl ProjectRefresh {
    pub(crate) fn begin(&mut self, signed_in: bool) -> Option<u64> {
        if !signed_in || self.in_flight {
            return None;
        }
        self.in_flight = true;
        Some(self.generation)
    }

    pub(crate) fn complete(&mut self, generation: u64) -> bool {
        if generation != self.generation || !self.in_flight {
            return false;
        }
        self.in_flight = false;
        true
    }

    pub(crate) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.in_flight = false;
    }
}

pub(crate) fn retained_selection<'a>(
    selected_id: Option<&str>,
    mut project_ids: impl Iterator<Item = &'a str>,
) -> Option<usize> {
    selected_id.and_then(|selected| project_ids.position(|id| id == selected))
}

#[cfg(test)]
mod tests {
    use super::{ProjectRefresh, retained_selection};

    #[test]
    fn signed_in_windows_can_refresh_repeatedly_without_restarting() {
        let mut refresh = ProjectRefresh::default();
        let first = refresh
            .begin(true)
            .expect("a signed-in window must refresh");
        assert_eq!(refresh.begin(true), None, "reads must not overlap");
        assert!(refresh.complete(first));
        assert!(
            refresh.begin(true).is_some(),
            "later membership changes need a new read"
        );
    }

    #[test]
    fn signed_out_windows_do_not_read_projects() {
        assert_eq!(ProjectRefresh::default().begin(false), None);
    }

    #[test]
    fn old_session_response_cannot_replace_new_session_projects() {
        let mut refresh = ProjectRefresh::default();
        let old = refresh.begin(true).unwrap();
        refresh.invalidate();
        let current = refresh.begin(true).unwrap();
        assert_ne!(old, current);
        assert!(!refresh.complete(old));
        assert_eq!(
            refresh.begin(true),
            None,
            "the current read is still pending"
        );
        assert!(refresh.complete(current));
    }

    #[test]
    fn new_memberships_and_renames_do_not_change_the_selected_project() {
        assert_eq!(
            retained_selection(Some("mine"), ["new", "mine"].into_iter()),
            Some(1)
        );
        assert_eq!(
            retained_selection(Some("mine"), ["mine", "other"].into_iter()),
            Some(0)
        );
    }

    #[test]
    fn organization_memory_and_removed_projects_do_not_select_a_different_project() {
        assert_eq!(retained_selection(None, ["new"].into_iter()), None);
        assert_eq!(
            retained_selection(Some("removed"), ["other"].into_iter()),
            None
        );
        assert_eq!(retained_selection(Some("removed"), [].into_iter()), None);
    }
}
