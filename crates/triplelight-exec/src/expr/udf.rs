//! Function callbacks supplied by an embedded database's captured registry.
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use triplelight_rdf::Term;

use crate::model::TermId;
use crate::sparql::Expression;
use crate::storage::{EntailmentProvider, Snapshot};
use crate::{Ctx, MEMO_CAP, MemoryBudget, Reservation};

/// A synchronous RDF scalar function. It may be called on an executor thread.
/// Return `None` to signal a SPARQL expression error. A callback must not mutate
/// or reenter the database evaluating it.
pub trait ScalarFunction: Send + Sync + 'static {
    fn call(&self, args: &[Term]) -> Option<Term>;
}

impl<F> ScalarFunction for F
where
    F: Fn(&[Term]) -> Option<Term> + Send + Sync + 'static,
{
    fn call(&self, args: &[Term]) -> Option<Term> {
        self(args)
    }
}

/// Declares a function's exact arity and whether its result depends only on its arguments.
#[derive(Debug, Default, Clone, Copy)]
pub struct FunctionOptions {
    pub arity: Option<usize>,
    pub pure: bool,
}

#[doc(hidden)]
#[derive(Clone)]
pub struct RegisteredFunction {
    callback: Arc<dyn ScalarFunction>,
    options: FunctionOptions,
}

impl RegisteredFunction {
    pub fn new(options: FunctionOptions, callback: impl ScalarFunction) -> Self {
        Self {
            options,
            callback: Arc::new(callback),
        }
    }

    pub(crate) fn accepts(&self, args: usize) -> bool {
        self.options.arity.is_none_or(|arity| arity == args)
    }

    pub(crate) fn pure(&self) -> bool {
        self.options.pure
    }

    pub(crate) fn call(&self, args: &[Term]) -> std::thread::Result<Option<Term>> {
        catch_unwind(AssertUnwindSafe(|| self.callback.call(args)))
    }
}

/// A synchronous accumulator for one aggregate occurrence in one group.
/// The evaluator skips unbound arguments and expression errors, so neither reaches `step`.
pub trait AggregateState {
    /// Processes one bound argument. `None` makes this group's result unbound.
    fn step(&mut self, term: &Term) -> Option<()>;

    /// Returns the group's result. `None` leaves the result unbound.
    fn finish(&mut self) -> Option<Term>;
}

/// A factory called on the query thread for each group, including an empty implicit group.
/// Callbacks must not reenter the evaluating database. Destructors must not panic.
pub trait AggregateFunction: Send + Sync + 'static {
    fn create(&self) -> Option<Box<dyn AggregateState>>;
}

impl<F> AggregateFunction for F
where
    F: Fn() -> Option<Box<dyn AggregateState>> + Send + Sync + 'static,
{
    fn create(&self) -> Option<Box<dyn AggregateState>> {
        self()
    }
}

#[doc(hidden)]
#[derive(Clone)]
pub struct RegisteredAggregate(Arc<dyn AggregateFunction>);

impl RegisteredAggregate {
    pub fn new(factory: impl AggregateFunction) -> Self {
        Self(Arc::new(factory))
    }

    pub(crate) fn create(&self) -> Option<Box<dyn AggregateState>> {
        self.0.create()
    }
}

/// Immutable lookup used by parsing, validation and evaluation of the same query.
#[doc(hidden)]
pub trait FunctionLookup: Sync {
    fn scalar(&self, iri: &str) -> Option<&RegisteredFunction>;

    fn aggregate(&self, _iri: &str) -> Option<&RegisteredAggregate> {
        None
    }
}

/// Extension providers borrowed from one captured registry.
#[doc(hidden)]
#[derive(Clone, Copy, Default)]
pub struct QueryExtensions<'a> {
    pub entailment: Option<&'a dyn EntailmentProvider>,
    pub functions: Option<&'a dyn FunctionLookup>,
    pub disable_function_memo: bool,
}

type Key = (usize, Vec<TermId>);
type Entry<'a> = (Option<Term>, Option<Reservation<'a>>);

#[derive(Default)]
pub(crate) struct Memo<'a>(HashMap<Key, Entry<'a>>);

impl<'a> Memo<'a> {
    pub(crate) fn get(&self, key: &Key) -> Option<Option<Term>> {
        self.0.get(key).map(|entry| entry.0.clone())
    }

    pub(crate) fn insert(
        &mut self,
        key: Key,
        term: &Option<Term>,
        budget: Option<&'a MemoryBudget>,
    ) {
        if self.0.len() >= MEMO_CAP {
            return;
        }
        let charge = if let Some(budget) = budget {
            let bytes = (std::mem::size_of::<(Key, Entry<'_>)>() * 2
                + key.1.capacity() * std::mem::size_of::<TermId>()
                + term.as_ref().map_or(0, |term| term.to_string().len()))
                as u64;
            let Ok(charge) = budget.try_charge(bytes) else {
                return;
            };
            Some(charge)
        } else {
            None
        };
        self.0.insert(key, (term.clone(), charge));
    }
}

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    pub(crate) fn call_scalar(
        &self,
        iri: &triplelight_rdf::NamedNode,
        args: &[Expression],
        evaluate: &dyn Fn(&Expression) -> Option<Term>,
    ) -> Option<Term> {
        let function = self.functions?.scalar(iri.as_str())?;
        if !function.accepts(args.len()) {
            return None;
        }
        self.check();
        let args: Vec<_> = args.iter().map(evaluate).collect::<Option<_>>()?;
        self.check();
        let memo = self.function_memo.as_ref().filter(|_| function.pure());
        // Use existing term IDs. A cache lookup must not allocate scratch IDs.
        let key = memo.and_then(|_| {
            let scratch = self.scratch.borrow();
            let ids = args
                .iter()
                .map(|arg| scratch.0.get(arg).copied().or_else(|| self.lookup(arg)))
                .collect::<Option<Vec<_>>>()?;
            Some((std::ptr::from_ref(function) as usize, ids))
        });
        if let (Some(memo), Some(key)) = (memo, &key)
            && let Some(value) = memo.borrow().get(key)
        {
            return value;
        }
        let result = function.call(&args);
        self.check();
        let result = result.ok()?;
        if let (Some(memo), Some(key)) = (memo, key) {
            memo.borrow_mut().insert(key, &result, self.budget);
        }
        result
    }
}
