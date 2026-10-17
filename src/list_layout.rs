//! Column-major Explorer list layout represented as a fixed-height row grid.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListCell {
    File(usize),
    Group(String),
    Empty,
}

/// Pack every group into adjacent columns, never below another group.
/// `per_column` counts the heading cell too and is clamped to two rows.
pub fn rows(groups: Vec<(String, Vec<usize>)>, per_column: usize) -> Vec<Vec<ListCell>> {
    let height = per_column.max(2);
    let mut output = vec![Vec::new(); height];
    for (name, files) in groups {
        let heading = !name.is_empty();
        let capacity = height - usize::from(heading);
        let column_count = files.len().div_ceil(capacity).max(usize::from(heading));
        for column in 0..column_count {
            for (row, cells) in output.iter_mut().enumerate() {
                let cell = if heading && row == 0 {
                    ListCell::Group(name.clone())
                } else {
                    let index = column * capacity + row - usize::from(heading);
                    files
                        .get(index)
                        .copied()
                        .map(ListCell::File)
                        .unwrap_or(ListCell::Empty)
                };
                cells.push(cell);
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_files_are_all_present_once_in_a_fixed_height_grid() {
        let grid = rows(
            vec![
                ("A".into(), vec![0, 1, 2, 3, 4]),
                ("B".into(), vec![5, 6, 7]),
            ],
            3,
        );
        assert_eq!(grid.len(), 3);
        assert!(grid.iter().all(|row| row.len() == 5));
        assert_eq!(
            grid[0],
            vec![
                ListCell::Group("A".into()),
                ListCell::Group("A".into()),
                ListCell::Group("A".into()),
                ListCell::Group("B".into()),
                ListCell::Group("B".into()),
            ]
        );
        let mut files: Vec<_> = grid
            .iter()
            .flatten()
            .filter_map(|cell| match cell {
                ListCell::File(index) => Some(*index),
                _ => None,
            })
            .collect();
        files.sort_unstable();
        assert_eq!(files, (0..8).collect::<Vec<_>>());
        assert_eq!(grid[2][2], ListCell::Empty);
        assert_eq!(grid[2][4], ListCell::Empty);
    }

    #[test]
    fn ungrouped_files_fill_columns_without_heading_space() {
        let grid = rows(vec![(String::new(), vec![0, 1, 2, 3, 4])], 3);
        assert_eq!(
            grid,
            vec![
                vec![ListCell::File(0), ListCell::File(3)],
                vec![ListCell::File(1), ListCell::File(4)],
                vec![ListCell::File(2), ListCell::Empty],
            ]
        );
    }

    #[test]
    fn tiny_viewports_keep_a_file_row_beside_each_group_heading() {
        let grid = rows(vec![("A".into(), vec![0, 1]), ("B".into(), vec![2])], 0);
        assert_eq!(grid.len(), 2);
        assert_eq!(
            grid[1],
            vec![ListCell::File(0), ListCell::File(1), ListCell::File(2)]
        );
    }
}
