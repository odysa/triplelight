//! Extend one solution with BIND or SELECT expressions.
use super::{Ctx, Row, UNBOUND, expr::BlankScope};
use crate::sparql::{Expression, Function, GraphPattern};
use crate::storage::Snapshot;
use triplelight_rdf::{Term, Variable};

fn has_named_blank(expression: &Expression) -> bool {
    matches!(expression, Expression::FunctionCall(Function::BNode, args) if args.len() == 1)
        || expression.children().any(has_named_blank)
}

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    pub(super) fn eval_extend(
        &self,
        inner: &GraphPattern,
        variable: &Variable,
        expression: &Expression,
        input: Vec<Row>,
    ) -> Vec<Row> {
        let column = self.var(variable);
        if !has_named_blank(expression) {
            return self
                .eval(inner, input)
                .into_iter()
                .filter_map(|mut row| {
                    let value = self.value(expression, &row);
                    self.bind_value(&mut row, column, value).then_some(row)
                })
                .collect();
        }

        // SELECT projections are consecutive Extend nodes. Evaluate them in order with one label
        // map for each occurrence, even when two input solutions have identical variable values.
        let mut bindings = vec![(column, expression)];
        let mut pattern = inner;
        while let GraphPattern::Extend {
            inner,
            variable,
            expression,
        } = pattern
        {
            bindings.push((self.var(variable), expression));
            pattern = inner;
        }
        self.eval(pattern, input)
            .into_iter()
            .enumerate()
            .filter_map(|(i, mut row)| {
                if i.is_multiple_of(4096) {
                    self.check();
                }
                let scope = BlankScope::default();
                bindings
                    .iter()
                    .rev()
                    .all(|&(column, expression)| {
                        let value = self.value_scoped(expression, &row, &scope);
                        self.bind_value(&mut row, column, value)
                    })
                    .then_some(row)
            })
            .collect()
    }

    fn bind_value(&self, row: &mut Row, column: usize, value: Option<Term>) -> bool {
        // A sideways outer binding must agree with the extension. An expression error preserves it.
        let Some(value) = value else { return true };
        let id = self.intern(&value);
        if row[column] == UNBOUND {
            row[column] = id;
        }
        row[column] == id
    }
}
