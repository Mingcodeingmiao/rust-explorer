//! Keep the longest fitting suffix; only leading ancestors go into overflow.
pub fn hidden_prefix(widths: &[f32], available: f32, overflow_width: f32) -> usize {
    let mut remaining: f32 = widths.iter().sum();
    if remaining <= available || widths.len() < 2 {
        return 0;
    }
    let mut hidden = 0;
    while hidden + 1 < widths.len() && remaining + overflow_width > available {
        remaining -= widths[hidden];
        hidden += 1;
    }
    hidden
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hides_only_the_minimum_leading_ancestors() {
        let widths = [70., 100., 80., 90.];
        assert_eq!(hidden_prefix(&widths, 340., 50.), 0);
        assert_eq!(hidden_prefix(&widths, 320., 50.), 1);
        assert_eq!(hidden_prefix(&widths, 250., 50.), 2);
        assert_eq!(hidden_prefix(&widths, 150., 50.), 3);
        assert_eq!(hidden_prefix(&widths, 20., 50.), 3);
        assert_eq!(hidden_prefix(&[500.], 80., 50.), 0);
    }
}
