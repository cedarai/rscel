use serde::{Deserialize, Serialize};

use super::{source_location::SourceLocation, source_range::SourceRange};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AstNode<T> {
    loc: SourceRange,

    node: T,
}

impl<T> AstNode<T> {
    pub fn new(node: T, loc: SourceRange) -> AstNode<T> {
        AstNode::<T> { loc, node }
    }

    pub fn into_parts(self) -> (T, SourceRange) {
        (self.node, self.loc)
    }

    pub fn start(&self) -> SourceLocation {
        self.loc.start()
    }

    pub fn end(&self) -> SourceLocation {
        self.loc.end()
    }

    pub fn range(&self) -> SourceRange {
        self.loc
    }

    pub fn node(&self) -> &T {
        &self.node
    }
}
