use crate::util::fmt_size;
use std::collections::HashMap;
use vaultpack_core::{Archive, Entry, EntryKind};

pub struct Node {
    pub name: String,
    pub icon: i32,
    pub children: Vec<usize>,
    pub entry: Option<usize>,
    pub start: usize,
    pub end: usize,
    pub size: u64,
    pub expanded: bool,
    lookup: HashMap<String, usize>,
}

pub struct RowOut {
    pub node: usize,
    pub depth: i32,
    pub state: i32,
    pub size: String,
}

pub struct Tree {
    pub nodes: Vec<Node>,
    pub order: Vec<usize>,
    pub selected: Vec<bool>,
    pub query: String,
    matches: Vec<bool>,
    pub visible: Vec<usize>,
}

fn group_title(g: &str) -> &'static str {
    if g.starts_with("browsers") {
        "Browser profiles"
    } else if g.starts_with("wifi") {
        "Wi-Fi networks"
    } else {
        "Installed programs"
    }
}

fn components(e: &Entry) -> (Vec<String>, i32) {
    if let Some(g) = &e.group {
        let parts: Vec<&str> = g.split('/').collect();
        let top = group_title(g).to_string();
        if g.starts_with("browsers") {
            let browser = parts.get(1).copied().unwrap_or("Browser").to_string();
            let profile = parts.get(2).copied().unwrap_or("Default").to_string();
            let segs: Vec<&str> = e.tpath.split('\\').filter(|s| !s.is_empty()).collect();
            let rel: Vec<String> = match segs.iter().position(|s| *s == profile) {
                Some(i) if i + 1 < segs.len() => segs[i + 1..].iter().map(|s| s.to_string()).collect(),
                _ => segs.last().map(|s| vec![s.to_string()]).unwrap_or_default(),
            };
            let mut v = vec![top, browser, profile];
            v.extend(rel);
            return (v, 2);
        }
        let name = parts.get(1).copied().unwrap_or("item").to_string();
        return (vec![top, name], if g.starts_with("wifi") { 3 } else { 4 });
    }
    let segs: Vec<String> = e.tpath.split('\\').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    (segs, if e.kind == EntryKind::Dir { 0 } else { 1 })
}

impl Tree {
    pub fn build(archive: &Archive) -> Tree {
        let mut nodes = vec![Node { name: String::new(), icon: 0, children: Vec::new(), entry: None, start: 0, end: 0, size: 0, expanded: true, lookup: HashMap::new() }];
        for (i, e) in archive.manifest.entries.iter().enumerate() {
            let (parts, leaf_icon) = components(e);
            if parts.is_empty() {
                continue;
            }
            let mut cur = 0usize;
            let last = parts.len() - 1;
            for (d, p) in parts.iter().enumerate() {
                let is_leaf = d == last;
                let existing = nodes[cur].lookup.get(p).copied();
                let idx = match existing {
                    Some(idx) if !(is_leaf && e.kind == EntryKind::File && nodes[idx].entry.is_some()) => idx,
                    _ => {
                        let icon = if is_leaf { leaf_icon } else if d == 0 && e.group.is_some() { leaf_icon.clamp(2, 4) } else { 0 };
                        nodes.push(Node { name: p.clone(), icon, children: Vec::new(), entry: None, start: 0, end: 0, size: 0, expanded: false, lookup: HashMap::new() });
                        let idx = nodes.len() - 1;
                        nodes[cur].children.push(idx);
                        nodes[cur].lookup.insert(p.clone(), idx);
                        idx
                    }
                };
                if is_leaf {
                    nodes[idx].entry = Some(i);
                    if e.kind == EntryKind::File {
                        nodes[idx].icon = leaf_icon;
                    }
                }
                cur = idx;
            }
        }
        let n = archive.manifest.entries.len();
        let mut t = Tree { nodes, order: Vec::new(), selected: vec![true; n], query: String::new(), matches: Vec::new(), visible: Vec::new() };
        t.finalize(0, archive);
        t.matches = vec![true; t.nodes.len()];
        t
    }

    fn finalize(&mut self, node: usize, archive: &Archive) {
        let mut kids = std::mem::take(&mut self.nodes[node].children);
        kids.sort_by(|&a, &b| {
            let (na, nb) = (&self.nodes[a], &self.nodes[b]);
            let ga = (na.icon >= 2) as u8;
            let gb = (nb.icon >= 2) as u8;
            gb.cmp(&ga).then((nb.children.is_empty()).cmp(&na.children.is_empty()).reverse()).then(na.name.to_lowercase().cmp(&nb.name.to_lowercase()))
        });
        self.nodes[node].start = self.order.len();
        if let Some(e) = self.nodes[node].entry {
            self.order.push(e);
        }
        for &k in &kids {
            self.finalize(k, archive);
        }
        self.nodes[node].children = kids;
        self.nodes[node].end = self.order.len();
        self.nodes[node].size = self.order[self.nodes[node].start..self.nodes[node].end].iter().map(|&i| archive.manifest.entries[i].size).sum();
    }

    pub fn set_query(&mut self, q: &str) {
        self.query = q.trim().to_lowercase();
        self.matches = vec![self.query.is_empty(); self.nodes.len()];
        if !self.query.is_empty() {
            self.mark(0);
        }
    }

    fn mark(&mut self, n: usize) -> bool {
        let mut hit = !self.query.is_empty() && self.nodes[n].name.to_lowercase().contains(&self.query);
        let kids = self.nodes[n].children.clone();
        for k in kids {
            if self.mark(k) {
                hit = true;
            }
        }
        self.matches[n] = hit;
        hit
    }

    pub fn rows(&mut self) -> Vec<RowOut> {
        let mut pre = vec![0usize; self.order.len() + 1];
        for (k, &i) in self.order.iter().enumerate() {
            pre[k + 1] = pre[k] + self.selected[i] as usize;
        }
        let mut out = Vec::new();
        self.visible.clear();
        let searching = !self.query.is_empty();
        let mut stack: Vec<(usize, i32)> = self.nodes[0].children.iter().rev().map(|&c| (c, 0)).collect();
        while let Some((n, depth)) = stack.pop() {
            if !self.matches[n] {
                continue;
            }
            let node = &self.nodes[n];
            let total = node.end - node.start;
            let sel = pre[node.end] - pre[node.start];
            let state = if total == 0 || sel == 0 { 0 } else if sel == total { 1 } else { 2 };
            out.push(RowOut { node: n, depth, state, size: if node.icon >= 3 && node.children.is_empty() { String::new() } else { fmt_size(node.size) } });
            self.visible.push(n);
            if (node.expanded || searching) && !node.children.is_empty() {
                for &c in node.children.iter().rev() {
                    stack.push((c, depth + 1));
                }
            }
        }
        out
    }

    pub fn toggle_check(&mut self, node: usize) {
        let (s, e) = (self.nodes[node].start, self.nodes[node].end);
        let all = self.order[s..e].iter().all(|&i| self.selected[i]);
        for k in s..e {
            let i = self.order[k];
            self.selected[i] = !all;
        }
    }

    pub fn bulk(&mut self, kind: i32) {
        match kind {
            0 => self.selected.iter_mut().for_each(|v| *v = true),
            1 => self.selected.iter_mut().for_each(|v| *v = false),
            2 => self.selected.iter_mut().for_each(|v| *v = !*v),
            3 => self.nodes.iter_mut().for_each(|n| n.expanded = !n.children.is_empty()),
            _ => self.nodes.iter_mut().skip(1).for_each(|n| n.expanded = false),
        }
    }

    pub fn toggle_expand(&mut self, node: usize) {
        self.nodes[node].expanded = !self.nodes[node].expanded;
    }

    pub fn selected_stats(&self, archive: &Archive) -> (usize, u64) {
        let mut c = 0;
        let mut s = 0;
        for (i, e) in archive.manifest.entries.iter().enumerate() {
            if self.selected[i] && e.kind == EntryKind::File {
                c += 1;
                s += e.size;
            }
        }
        (c, s)
    }

    pub fn selected_set(&self) -> std::collections::HashSet<usize> {
        self.selected.iter().enumerate().filter(|(_, &v)| v).map(|(i, _)| i).collect()
    }
}
