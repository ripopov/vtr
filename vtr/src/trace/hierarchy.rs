use super::Reader;
use crate::{
    Error, Result,
    declarations::{Node, NodeData},
    hierarchy::NodeId,
};

pub(super) struct Tree {
    offsets: Vec<usize>,
    children: Vec<NodeId>,
}
impl Tree {
    pub fn new(nodes: &[Node]) -> Self {
        let mut offsets = vec![0; nodes.len() + 2];
        for node in nodes {
            offsets[node.parent.map_or(0, |parent| parent.0 as usize + 1) + 1] += 1;
        }
        for index in 1..offsets.len() {
            offsets[index] += offsets[index - 1];
        }
        let mut next = offsets.clone();
        let mut children = vec![NodeId(0); nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            let bucket = node.parent.map_or(0, |parent| parent.0 as usize + 1);
            children[next[bucket]] = NodeId(index as u32);
            next[bucket] += 1;
        }
        Self { offsets, children }
    }
    fn children(&self, parent: Option<NodeId>) -> &[NodeId] {
        let bucket = parent.map_or(0, |parent| parent.0 as usize + 1);
        match (self.offsets.get(bucket), self.offsets.get(bucket + 1)) {
            (Some(&start), Some(&end)) => &self.children[start..end],
            _ => &[],
        }
    }
}
impl Reader {
    pub fn node(&self, id: NodeId) -> Result<&Node> {
        self.declarations
            .nodes
            .get(id.0 as usize)
            .ok_or(Error::invalid("unknown hierarchy node"))
    }
    pub fn name(&self, id: NodeId) -> Result<&str> {
        self.string(self.node(id)?.name)
    }
    #[must_use]
    pub fn children(&self, parent: Option<NodeId>) -> &[NodeId] {
        self.tree.children(parent)
    }
    pub fn full_path(&self, id: NodeId) -> Result<String> {
        let mut names = Vec::new();
        let mut current = Some(id);
        while let Some(id) = current {
            let node = self.node(id)?;
            names.push(self.string(node.name)?);
            current = node.parent;
        }
        names.reverse();
        Ok(names.join("."))
    }
    #[must_use]
    pub fn is_within(&self, node: NodeId, scope: Option<NodeId>) -> bool {
        let Some(scope) = scope else {
            return true;
        };
        let mut current = Some(node);
        while let Some(id) = current {
            if id == scope {
                return true;
            }
            current = self
                .declarations
                .nodes
                .get(id.0 as usize)
                .and_then(|node| node.parent);
        }
        false
    }
    #[must_use]
    pub fn scope_sizes(&self) -> (Vec<NodeId>, crate::census::ScopeSizes) {
        let mut census = crate::census::Census::new();
        let mut nodes = Vec::new();
        let mut stack: Vec<_> = self
            .children(None)
            .iter()
            .rev()
            .filter(|id| {
                matches!(
                    self.declarations.nodes[id.0 as usize].data,
                    NodeData::Scope { .. }
                )
            })
            .map(|id| Some(*id))
            .collect();
        while let Some(entry) = stack.pop() {
            let Some(id) = entry else {
                census.leave();
                continue;
            };
            census.enter();
            nodes.push(id);
            stack.push(None);
            for &child in self.children(Some(id)) {
                if let NodeData::Signal { signal, .. } =
                    self.declarations.nodes[child.0 as usize].data
                {
                    census.var(signal.0);
                }
            }
            stack.extend(
                self.children(Some(id))
                    .iter()
                    .rev()
                    .filter(|id| {
                        matches!(
                            self.declarations.nodes[id.0 as usize].data,
                            NodeData::Scope { .. }
                        )
                    })
                    .map(|id| Some(*id)),
            );
        }
        (nodes, census.finish())
    }
}
