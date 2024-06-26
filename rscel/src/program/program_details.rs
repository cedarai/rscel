use crate::{
    compiler::{ast_node::AstNode, grammar::Expr},
    utils::IdentFilterIter,
    BindContext,
};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde_as]
pub struct ProgramDetails {
    source: Option<String>,
    params: HashSet<String>,
    #[serde(skip_serializing, skip_deserializing)]
    ast: Option<AstNode<Expr>>,
}

impl ProgramDetails {
    pub fn new() -> ProgramDetails {
        ProgramDetails {
            source: None,
            params: HashSet::new(),
            ast: None,
        }
    }

    pub fn joined2(mut pd1: ProgramDetails, pd2: ProgramDetails) -> ProgramDetails {
        pd1.union_from(pd2);
        pd1
    }

    pub fn add_ast(&mut self, ast: AstNode<Expr>) {
        self.ast = Some(ast);
    }

    pub fn add_source(&mut self, source: String) {
        self.source = Some(source);
    }

    pub fn union_from(&mut self, other: ProgramDetails) {
        for param in other.params.iter() {
            self.params.insert(param.to_string());
        }
    }

    pub fn ast<'a>(&'a self) -> Option<&'a AstNode<Expr>> {
        self.ast.as_ref()
    }

    pub fn source<'a>(&'a self) -> Option<&'a str> {
        self.source.as_deref()
    }

    pub fn add_param(&mut self, name: &str) {
        self.params.insert(name.to_owned());
    }

    pub fn params<'a>(&'a self) -> Vec<&'a str> {
        self.params.iter().map(|x| x.as_str()).collect()
    }

    pub fn filter_from_bindings(&mut self, bindings: &BindContext) {
        self.params =
            IdentFilterIter::new(bindings, &mut self.params.iter().map(|x| x.as_str())).collect();
    }
}
