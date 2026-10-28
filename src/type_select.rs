//! Incremental filename selection in visible display order.
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct TypeSelect {
    prefix: String,
    last_input: Option<Instant>,
}
impl TypeSelect {
    pub fn clear(&mut self) {
        self.prefix.clear();
        self.last_input = None;
    }
    pub fn find(
        &mut self,
        ch: char,
        now: Instant,
        order: &[usize],
        current: Option<usize>,
        name: impl Fn(usize) -> String,
    ) -> Option<usize> {
        if self
            .last_input
            .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_secs(1))
        {
            self.clear();
        }
        let typed = ch.to_lowercase().collect::<String>();
        let cycle = self.prefix == typed;
        let continuing = !self.prefix.is_empty() && !cycle;
        if !cycle {
            self.prefix.push_str(&typed);
        }
        self.last_input = Some(now);
        let position = current.and_then(|i| order.iter().position(|v| *v == i));
        let start = position.map_or(0, |p| if continuing { p } else { p + 1 });
        (0..order.len())
            .map(|offset| order[(start + offset) % order.len()])
            .find(|i| name(*i).to_lowercase().starts_with(&self.prefix))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn find(s: &mut TypeSelect, ch: char, now: Instant, current: Option<usize>) -> Option<usize> {
        let names = ["apple", "Archive", "readme", "resources", "9notes"];
        s.find(ch, now, &[0, 1, 2, 3, 4], current, |i| names[i].into())
    }
    #[test]
    fn cycles_and_wraps() {
        let mut s = TypeSelect::default();
        let now = Instant::now();
        assert_eq!(find(&mut s, 'A', now, None), Some(0));
        assert_eq!(find(&mut s, 'a', now, Some(0)), Some(1));
        assert_eq!(find(&mut s, 'a', now, Some(1)), Some(0));
    }
    #[test]
    fn prefixes_and_timeout() {
        let mut s = TypeSelect::default();
        let now = Instant::now();
        assert_eq!(find(&mut s, 'r', now, None), Some(2));
        assert_eq!(find(&mut s, 'e', now, Some(2)), Some(2));
        assert_eq!(find(&mut s, 's', now, Some(2)), Some(3));
        assert_eq!(
            find(&mut s, '9', now + Duration::from_secs(1), Some(3)),
            Some(4)
        );
    }
    #[test]
    fn visible_order_and_no_match() {
        let mut s = TypeSelect::default();
        let now = Instant::now();
        assert_eq!(
            s.find('a', now, &[2, 0], Some(2), |i| ["apple", "archive", "beta"]
                [i]
                .into()),
            Some(0)
        );
        s.clear();
        assert_eq!(find(&mut s, 'z', now, None), None);
        assert_eq!(s.find('a', now, &[], None, |_| unreachable!()), None);
    }
}
