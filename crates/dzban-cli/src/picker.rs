#[derive(Default)]
pub struct Picker {
    pub search: String,
    selected: usize,
}

impl Picker {
    pub fn visible_items(&self, items: &[String], capacity: usize) -> Vec<usize> {
        let matches = self.matching_items(items);
        let capacity = capacity.max(1);
        let selected = self.selected.min(matches.len().saturating_sub(1));
        let start = selected.saturating_sub(capacity - 1);
        matches.into_iter().skip(start).take(capacity).collect()
    }
    pub fn matching_items(&self, items: &[String]) -> Vec<usize> {
        let search = self.search.to_lowercase();
        items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.to_lowercase().contains(&search))
            .map(|(index, _)| index)
            .collect()
    }
    pub fn selected_item(&self, items: &[String]) -> Option<usize> {
        let matches = self.matching_items(items);
        matches
            .get(self.selected.min(matches.len().saturating_sub(1)))
            .copied()
    }
    pub fn move_item(&mut self, delta: isize, items: &[String]) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.matching_items(items).len().saturating_sub(1));
    }
    pub fn search(&mut self, text: &str) {
        self.search.push_str(text);
        self.selected = 0;
    }
    pub fn backspace(&mut self) {
        self.search.pop();
        self.selected = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_keeps_selection_visible_within_terminal_height() {
        let items = (0..100).map(|index| index.to_string()).collect::<Vec<_>>();
        let mut picker = Picker::default();
        assert_eq!(picker.visible_items(&items, 3), [0, 1, 2]);
        picker.move_item(99, &items);
        assert_eq!(picker.visible_items(&items, 3), [97, 98, 99]);
        assert_eq!(picker.visible_items(&items, 0), [99]);
        picker.search("missing");
        assert!(picker.visible_items(&items, 3).is_empty());
    }

    #[test]
    fn filters_navigation_and_empty_selection_are_stable() {
        let profiles = ["local", "production"].map(str::to_owned);
        let mut picker = Picker::default();
        assert_eq!(picker.selected_item(&profiles), Some(0));
        picker.move_item(10, &profiles);
        assert_eq!(picker.selected_item(&profiles), Some(1));
        picker.search("local");
        assert_eq!(picker.selected_item(&profiles), Some(0));
        picker.search("missing");
        assert_eq!(picker.selected_item(&profiles), None);
    }

    #[test]
    fn history_search_is_case_insensitive_and_keeps_original_indices() {
        let entries = ["SELECT 1", "SELECT 'žbán'", "select 2"].map(str::to_owned);
        let mut picker = Picker::default();
        picker.search("SELECT");
        assert_eq!(picker.matching_items(&entries), [0, 1, 2]);
        picker.move_item(1, &entries);
        assert_eq!(picker.selected_item(&entries), Some(1));
        picker.search(" 'ŽBÁN'");
        assert_eq!(picker.selected_item(&entries), Some(1));
        picker.search("not found");
        assert_eq!(picker.selected_item(&entries), None);
    }
}
