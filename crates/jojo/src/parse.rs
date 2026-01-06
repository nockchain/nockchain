#[derive(Debug, PartialEq)]
pub enum Node {
    Leaf(String),
    Branch(Vec<Node>),
}

impl Node {
    pub fn fold<T, E, C>(
        self,
        c: &mut C,
        fold_leaf: &mut impl FnMut(String, &mut C) -> Result<T, E>,
        fold_branch: &mut impl FnMut(Vec<T>, &mut C) -> Result<T, E>,
    ) -> Result<T, E> {
        match self {
            Self::Leaf(l) => fold_leaf(l, c),
            Self::Branch(b) => {
                let b = b
                    .into_iter()
                    .map(|v| v.fold(c, fold_leaf, fold_branch))
                    .collect::<Result<Vec<T>, E>>()?;
                fold_branch(b, c)
            }
        }
    }
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    pub fn new(input: &str) -> Self {
        Parser {
            chars: input.chars().collect::<Vec<_>>(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) {
        self.pos += 1;
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.bump();
        }
    }

    fn parse_leaf(&mut self) -> Node {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if !c.is_whitespace() && c != ']' && c != '[') {
            self.bump();
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        Node::Leaf(s)
    }

    fn parse_branch(&mut self) -> Option<Node> {
        // consume '['
        assert_eq!(self.peek(), Some('['));
        self.bump();
        let mut children = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(']') => {
                    self.bump();
                    break;
                }
                Some('[') => children.push(self.parse_branch()?),
                Some(_) => children.push(self.parse_leaf()),
                None => return None,
            }
        }
        Some(Node::Branch(children))
    }

    fn parse(&mut self) -> Option<Node> {
        self.skip_ws();
        match self.peek() {
            Some('[') => self.parse_branch(),
            Some(_) => Some(self.parse_leaf()),
            None => None,
        }
    }
}

pub fn parse_tree(input: &str) -> Option<(Node, &str)> {
    let mut parser = Parser::new(input);
    let parsed = parser.parse()?;
    Some((parsed, input.split_at(parser.pos).1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple() {
        assert_eq!(parse_tree("a").unwrap(), (Node::Leaf("a".into()), ""));
        assert_eq!(
            parse_tree("[a b c]").unwrap(),
            (
                Node::Branch(vec![
                    Node::Leaf("a".into()),
                    Node::Leaf("b".into()),
                    Node::Leaf("c".into()),
                ]),
                ""
            )
        );
        assert_eq!(
            parse_tree("[[a b] c] asd").unwrap(),
            (
                Node::Branch(vec![
                    Node::Branch(vec![Node::Leaf("a".into()), Node::Leaf("b".into())]),
                    Node::Leaf("c".into()),
                ]),
                " asd"
            )
        );
        assert_eq!(
            parse_tree("[[a b c] d]").unwrap().0,
            Node::Branch(vec![
                Node::Branch(vec![
                    Node::Leaf("a".into()),
                    Node::Leaf("b".into()),
                    Node::Leaf("c".into()),
                ]),
                Node::Leaf("d".into()),
            ])
        );
        assert_eq!(
            parse_tree("[[a [b c]] d]").unwrap().0,
            Node::Branch(vec![
                Node::Branch(vec![
                    Node::Leaf("a".into()),
                    Node::Branch(vec![Node::Leaf("b".into()), Node::Leaf("c".into()),]),
                ]),
                Node::Leaf("d".into()),
            ])
        );
    }
}
