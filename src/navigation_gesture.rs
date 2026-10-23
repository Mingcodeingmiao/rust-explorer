/// GPUI counts all fast clicks at the same position, including across folders.
/// Treat 2, 4, 6... as double clicks instead of getting stuck after click two.
pub fn opens_on_click(count: usize) -> bool {
    count > 0 && count % 2 == 0
}

#[cfg(test)]
mod tests {
    #[test]
    fn stationary_clicks_keep_opening_one_folder_per_pair() {
        assert_eq!(
            (1..=12)
                .filter(|n| super::opens_on_click(*n))
                .collect::<Vec<_>>(),
            vec![2, 4, 6, 8, 10, 12]
        );
        assert!(!super::opens_on_click(0));
    }
}
