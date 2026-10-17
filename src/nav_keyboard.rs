//! Navigation-tree key rules, independent of GPUI and filesystem I/O.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub depth: usize,
    pub selectable: bool,
    pub expandable: bool,
    pub open: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Up,
    Down,
    First,
    Last,
    Left,
    Right,
    Expand,
    Collapse,
    ExpandLevel,
    Activate,
    ContextMenu,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Focus(usize),
    Expand(usize),
    Collapse(usize),
    ExpandLevel(usize),
    Activate(usize),
    None,
}
pub fn direct_children(nodes: &[Node], index: usize, limit: usize) -> Vec<usize> {
    let Some(parent) = nodes.get(index) else {
        return vec![];
    };
    nodes
        .iter()
        .enumerate()
        .skip(index + 1)
        .take_while(|(_, node)| node.depth > parent.depth)
        .filter(|(_, node)| node.depth == parent.depth + 1 && node.expandable && node.selectable)
        .take(limit)
        .map(|(index, _)| index)
        .collect()
}
pub fn apply(nodes: &[Node], index: usize, command: Command) -> Effect {
    let Some(current) = nodes.get(index) else {
        return nodes
            .iter()
            .position(|n| n.selectable)
            .map(Effect::Focus)
            .unwrap_or(Effect::None);
    };
    let first = || nodes.iter().position(|n| n.selectable);
    let last = || nodes.iter().rposition(|n| n.selectable);
    let focus = |index: Option<usize>| index.map(Effect::Focus).unwrap_or(Effect::None);
    match command {
        Command::First => focus(first()),
        Command::Last => focus(last()),
        Command::Up => focus((0..index).rev().find(|i| nodes[*i].selectable)),
        Command::Down => focus((index + 1..nodes.len()).find(|i| nodes[*i].selectable)),
        Command::Right if current.expandable && !current.open => Effect::Expand(index),
        Command::Right if current.expandable && current.open => focus(
            nodes
                .get(index + 1)
                .filter(|n| n.selectable && n.depth > current.depth)
                .map(|_| index + 1),
        ),
        Command::Left if current.expandable && current.open => Effect::Collapse(index),
        Command::Left => focus(
            (0..index)
                .rev()
                .find(|i| nodes[*i].selectable && nodes[*i].depth < current.depth),
        ),
        Command::Expand if current.expandable && !current.open => Effect::Expand(index),
        Command::Collapse if current.expandable && current.open => Effect::Collapse(index),
        Command::ExpandLevel if current.expandable => Effect::ExpandLevel(index),
        Command::Activate if current.selectable => Effect::Activate(index),
        _ => Effect::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(depth: usize, selectable: bool, expandable: bool, open: bool) -> Node {
        Node {
            depth,
            selectable,
            expandable,
            open,
        }
    }
    #[test]
    fn skips_separators_and_clamps_at_visible_ends() {
        let nodes = [
            node(0, true, false, false),
            node(0, false, false, false),
            node(0, true, false, false),
        ];
        assert_eq!(apply(&nodes, 0, Command::Down), Effect::Focus(2));
        assert_eq!(apply(&nodes, 2, Command::Up), Effect::Focus(0));
        assert_eq!(apply(&nodes, 2, Command::Down), Effect::None);
        assert_eq!(apply(&nodes, 2, Command::First), Effect::Focus(0));
        assert_eq!(apply(&nodes, 0, Command::Last), Effect::Focus(2));
    }
    #[test]
    fn arrows_expand_then_select_children_and_collapse_then_select_parent() {
        let nodes = [
            node(0, true, true, true),
            node(1, true, true, false),
            node(0, true, true, false),
        ];
        assert_eq!(apply(&nodes, 0, Command::Right), Effect::Focus(1));
        assert_eq!(apply(&nodes, 1, Command::Right), Effect::Expand(1));
        assert_eq!(apply(&nodes, 1, Command::Left), Effect::Focus(0));
        assert_eq!(apply(&nodes, 0, Command::Left), Effect::Collapse(0));
        assert_eq!(apply(&nodes, 2, Command::Right), Effect::Expand(2));
        assert_eq!(
            apply(&nodes, 1, Command::ExpandLevel),
            Effect::ExpandLevel(1)
        );
    }
    #[test]
    fn quick_access_leaf_never_expands() {
        let nodes = [node(0, true, true, true), node(1, true, false, false)];
        assert_eq!(apply(&nodes, 1, Command::Right), Effect::None);
        assert_eq!(apply(&nodes, 1, Command::ExpandLevel), Effect::None);
        assert_eq!(apply(&nodes, 1, Command::Activate), Effect::Activate(1));
    }
    #[test]
    fn star_expands_only_a_bounded_direct_level() {
        let nodes = [
            node(0, true, true, true),
            node(1, true, true, true),
            node(2, true, true, false),
            node(1, true, true, false),
            node(1, true, true, false),
            node(0, true, true, false),
        ];
        assert_eq!(direct_children(&nodes, 0, 2), vec![1, 3]);
        assert_eq!(direct_children(&nodes, 0, 16), vec![1, 3, 4]);
    }
}
