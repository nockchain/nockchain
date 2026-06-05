use super::*;

pub trait TraceFilter: Send {
    fn should_trace(&mut self, path: Noun) -> bool;

    fn compose<B: TraceFilter, F: Fn(bool, bool) -> bool>(
        self,
        b: B,
        f: F,
    ) -> ComposeFilter<Self, B, F>
    where
        Self: Sized,
    {
        ComposeFilter(self, b, f)
    }

    fn and<B: TraceFilter>(self, b: B) -> ComposeFilter<Self, B, fn(bool, bool) -> bool>
    where
        Self: Sized,
    {
        self.compose(b, |a, b| a && b)
    }

    fn or<B: TraceFilter>(self, b: B) -> ComposeFilter<Self, B, fn(bool, bool) -> bool>
    where
        Self: Sized,
    {
        self.compose(b, |a, b| a || b)
    }

    fn boxed(self) -> Box<dyn TraceFilter>
    where
        Self: Sized + 'static,
    {
        Box::new(self)
    }
}

pub struct ComposeFilter<A, B, F: Fn(bool, bool) -> bool>(pub A, pub B, pub F);

impl<A: TraceFilter, B: TraceFilter, F: Fn(bool, bool) -> bool + Send> TraceFilter
    for ComposeFilter<A, B, F>
{
    fn should_trace(&mut self, path: Noun) -> bool {
        let a = self.0.should_trace(path);
        let b = self.1.should_trace(path);
        (self.2)(a, b)
    }
}

pub struct KeywordFilter<T> {
    pub keywords: Vec<T>,
}

impl<T: AsRef<str> + Send> TraceFilter for KeywordFilter<T> {
    fn should_trace(&mut self, path: Noun) -> bool {
        fn has_keywords(n: Noun, cnt: usize, kw: &[impl AsRef<str>]) -> bool {
            if cnt == 0 {
                return false;
            }
            if let Ok(n) = n.as_atom() {
                let b = n.as_ne_bytes();
                let b = &b[..b.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1)];
                return kw.iter().map(|v| v.as_ref()).any(|v| v.as_bytes() == b);
            }
            if let Ok(c) = n.as_cell() {
                return has_keywords(c.head(), cnt - 1, kw) || has_keywords(c.tail(), cnt - 1, kw);
            }
            false
        }

        has_keywords(path, 10, &self.keywords)
    }
}

pub struct IntervalFilter {
    pub interval: usize,
    pub cnt: usize,
}

impl TraceFilter for IntervalFilter {
    fn should_trace(&mut self, _: Noun) -> bool {
        let c = self.cnt;
        self.cnt += 1;
        c.is_multiple_of(self.interval)
    }
}
