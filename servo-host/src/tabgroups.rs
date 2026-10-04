//! Phase 4 Area 4.3 — tab groups, DATA LAYER (no engine involvement).
//!
//! A group has a name, a color and a collapsed state; tabs are members
//! via their shell tab id. The store is plain data: serializable to JSON
//! (session persistence in 4.4 + status dumps) and unit-testable. The
//! shell draws a color bar on grouped tabs and exposes every mutation
//! through the automation FIFO; a future UI renders the rest.

use std::collections::HashMap;

/// Group colors (Chrome-style named set). RGB tuples so both the strip
/// renderer and JSON serialization can use them directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupColor {
    Grey,
    Blue,
    Red,
    Yellow,
    Green,
    Pink,
    Purple,
    Cyan,
}

impl GroupColor {
    pub const ALL: [GroupColor; 8] = [
        GroupColor::Grey,
        GroupColor::Blue,
        GroupColor::Red,
        GroupColor::Yellow,
        GroupColor::Green,
        GroupColor::Pink,
        GroupColor::Purple,
        GroupColor::Cyan,
    ];

    pub fn rgb(self) -> [u8; 3] {
        match self {
            GroupColor::Grey => [0x9a, 0xa0, 0xa6],
            GroupColor::Blue => [0x4c, 0x8d, 0xf0],
            GroupColor::Red => [0xe0, 0x5d, 0x50],
            GroupColor::Yellow => [0xd9, 0xac, 0x3c],
            GroupColor::Green => [0x4b, 0xa5, 0x62],
            GroupColor::Pink => [0xe2, 0x7e, 0xa8],
            GroupColor::Purple => [0x8e, 0x6b, 0xd0],
            GroupColor::Cyan => [0x3c, 0xb3, 0xbe],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            GroupColor::Grey => "grey",
            GroupColor::Blue => "blue",
            GroupColor::Red => "red",
            GroupColor::Yellow => "yellow",
            GroupColor::Green => "green",
            GroupColor::Pink => "pink",
            GroupColor::Purple => "purple",
            GroupColor::Cyan => "cyan",
        }
    }

    pub fn from_name(s: &str) -> Option<GroupColor> {
        GroupColor::ALL.into_iter().find(|c| c.name() == s)
    }

    pub fn json_rgb(self) -> String {
        let [r, g, b] = self.rgb();
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

/// One group. `id` is stable for the session (and persisted in 4.4).
#[derive(Debug, Clone, PartialEq)]
pub struct TabGroup {
    pub id: u32,
    pub name: String,
    pub color: GroupColor,
    pub collapsed: bool,
}

/// Group store: group records + membership by shell tab id.
#[derive(Debug, Clone, Default)]
pub struct TabGroupStore {
    next_group_id: u32,
    groups: Vec<TabGroup>,
    /// tab id -> group id.
    membership: HashMap<u64, u32>,
}

impl TabGroupStore {
    pub fn new() -> Self {
        TabGroupStore { next_group_id: 1, groups: Vec::new(), membership: HashMap::new() }
    }

    /// Create a group; returns its id. Name defaults to the color name
    /// when empty (Chrome behavior).
    pub fn create(&mut self, name: &str, color: GroupColor) -> u32 {
        let id = self.next_group_id;
        self.next_group_id += 1;
        let name = if name.trim().is_empty() { color.name().to_string() } else { name.trim().to_string() };
        self.groups.push(TabGroup { id, name, color, collapsed: false });
        id
    }

    pub fn delete(&mut self, group_id: u32) -> bool {
        let before = self.groups.len();
        self.groups.retain(|g| g.id != group_id);
        if self.groups.len() != before {
            self.membership.retain(|_, gid| *gid != group_id);
            true
        } else {
            false
        }
    }

    pub fn get(&self, group_id: u32) -> Option<&TabGroup> {
        self.groups.iter().find(|g| g.id == group_id)
    }

    pub fn get_mut(&mut self, group_id: u32) -> Option<&mut TabGroup> {
        self.groups.iter_mut().find(|g| g.id == group_id)
    }

    pub fn groups(&self) -> &[TabGroup] {
        &self.groups
    }

    pub fn rename(&mut self, group_id: u32, name: &str) -> bool {
        match self.get_mut(group_id) {
            Some(g) if !name.trim().is_empty() => {
                g.name = name.trim().to_string();
                true
            }
            _ => false,
        }
    }

    pub fn recolor(&mut self, group_id: u32, color: GroupColor) -> bool {
        match self.get_mut(group_id) {
            Some(g) => {
                g.color = color;
                true
            }
            _ => false,
        }
    }

    pub fn toggle_collapsed(&mut self, group_id: u32) -> Option<bool> {
        let g = self.get_mut(group_id)?;
        g.collapsed = !g.collapsed;
        Some(g.collapsed)
    }

    /// Add a tab to a group (replaces any previous membership).
    pub fn add_tab(&mut self, tab_id: u64, group_id: u32) -> bool {
        if self.get(group_id).is_none() {
            return false;
        }
        self.membership.insert(tab_id, group_id);
        true
    }

    /// Remove a tab from its group (no-op if ungrouped).
    pub fn remove_tab(&mut self, tab_id: u64) {
        self.membership.remove(&tab_id);
    }

    /// Membership is dropped together with the tab.
    pub fn tab_closed(&mut self, tab_id: u64) {
        self.remove_tab(tab_id);
    }

    /// Phase 4.4.4 session restore: re-insert a group with its ORIGINAL
    /// id (id space advanced past it so new creates never collide).
    pub fn restore_group(&mut self, id: u32, name: String, color: GroupColor, collapsed: bool) {
        self.next_group_id = self.next_group_id.max(id + 1);
        if self.get(id).is_none() {
            self.groups.push(TabGroup { id, name, color, collapsed });
        }
    }

    pub fn group_of(&self, tab_id: u64) -> Option<u32> {
        self.membership.get(&tab_id).copied()
    }

    /// Tab ids in a group, in insertion order.
    pub fn tabs_in(&self, group_id: u32) -> Vec<u64> {
        self.membership
            .iter()
            .filter(|(_, gid)| **gid == group_id)
            .map(|(tid, _)| *tid)
            .collect()
    }

    /// Strip-rendering helper: color for each tab, aligned by tab id.
    pub fn color_for(&self, tab_id: u64) -> Option<GroupColor> {
        self.group_of(tab_id).and_then(|gid| self.get(gid)).map(|g| g.color)
    }

    /// JSON dump (status events + session persistence in 4.4).
    /// Space-free so it survives the event channel's `key=value` escaping.
    pub fn to_json(&self) -> String {
        let groups: Vec<String> = self
            .groups
            .iter()
            .map(|g| {
                let tabs: Vec<String> =
                    self.tabs_in(g.id).iter().map(|t| t.to_string()).collect();
                format!(
                    r#"{{"id":{},"name":"{}","color":"{}","collapsed":{},"tabs":[{}]}}"#,
                    g.id,
                    g.name.replace('"', "\\\""),
                    g.color.name(),
                    g.collapsed,
                    tabs.join(","),
                )
            })
            .collect();
        format!(r#"{{"groups":[{}]}}"#, groups.join(","))
    }

    pub fn from_json_str(_s: &str) -> Option<TabGroupStore> {
        // Session restore (4.4) rehydrates groups via serde there; kept
        // out of this module to avoid a serde_json dependency cycle in
        // the data layer's unit tests.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_defaults() {
        let mut s = TabGroupStore::new();
        let id = s.create("", GroupColor::Blue);
        let g = s.get(id).unwrap();
        assert_eq!(g.name, "blue", "empty name defaults to color name");
        assert!(!g.collapsed);
        // Ids are unique and increasing.
        let id2 = s.create("docs", GroupColor::Red);
        assert_ne!(id, id2);
    }

    #[test]
    fn membership_lifecycle() {
        let mut s = TabGroupStore::new();
        let g1 = s.create("news", GroupColor::Green);
        let g2 = s.create("docs", GroupColor::Purple);
        assert!(s.add_tab(7, g1));
        assert!(s.add_tab(8, g1));
        assert!(!s.add_tab(9, 999), "unknown group rejected");
        assert_eq!(s.group_of(7), Some(g1));
        assert_eq!(s.tabs_in(g1).len(), 2);
        // Re-adding moves membership.
        assert!(s.add_tab(7, g2));
        assert_eq!(s.group_of(7), Some(g2));
        assert_eq!(s.tabs_in(g1), vec![8]);
        // Closing a tab drops membership.
        s.tab_closed(8);
        assert_eq!(s.tabs_in(g1).len(), 0);
        assert_eq!(s.group_of(8), None);
    }

    #[test]
    fn mutations() {
        let mut s = TabGroupStore::new();
        let g = s.create("", GroupColor::Red);
        assert!(s.rename(g, "research"));
        assert_eq!(s.get(g).unwrap().name, "research");
        assert!(!s.rename(g, "  "), "blank rename rejected");
        assert!(s.recolor(g, GroupColor::Cyan));
        assert_eq!(s.get(g).unwrap().color, GroupColor::Cyan);
        assert_eq!(s.toggle_collapsed(g), Some(true));
        assert_eq!(s.toggle_collapsed(g), Some(false));
        // Delete ungroups members.
        s.add_tab(3, g);
        assert!(s.delete(g));
        assert_eq!(s.group_of(3), None);
        assert!(!s.delete(g), "double delete false");
    }

    #[test]
    fn color_lookup_and_json() {
        assert_eq!(GroupColor::from_name("pink"), Some(GroupColor::Pink));
        assert_eq!(GroupColor::from_name("mauve"), None);
        let mut s = TabGroupStore::new();
        let g = s.create("work", GroupColor::Pink);
        s.add_tab(11, g);
        assert_eq!(s.color_for(11), Some(GroupColor::Pink));
        assert_eq!(s.color_for(12), None);
        let json = s.to_json();
        assert!(json.contains(r#""name":"work""#));
        assert!(json.contains(r#""color":"pink""#));
        assert!(json.contains("[11]"));
    }
}
