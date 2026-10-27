//! Turning History-tab gestures into the step order given to
//! [`crate::Document::rewrite_history`]: dragging a row elsewhere, or
//! deleting the ticked rows.

use std::collections::BTreeSet;

/// The order after dragging the step at `from` so that it lands before the
/// row `before` (`len` = after the last row). `None` when nothing moves.
#[must_use]
pub fn moved(len: usize, from: usize, before: usize) -> Option<Vec<usize>> {
    if from >= len || before > len || before == from || before == from + 1 {
        return None;
    }
    let mut order: Vec<usize> = (0..len).collect();
    let step = order.remove(from);
    let at = if before > from { before - 1 } else { before };
    order.insert(at, step);
    Some(order)
}

/// The order keeping every step but the `removed` ones. `None` when
/// nothing would be removed.
#[must_use]
pub fn without(len: usize, removed: &BTreeSet<usize>) -> Option<Vec<usize>> {
    let order: Vec<usize> = (0..len).filter(|i| !removed.contains(i)).collect();
    (order.len() < len).then_some(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_down_and_up_moves_one_step() {
        assert_eq!(moved(4, 0, 3), Some(vec![1, 2, 0, 3]));
        assert_eq!(moved(4, 0, 4), Some(vec![1, 2, 3, 0]));
        assert_eq!(moved(4, 3, 0), Some(vec![3, 0, 1, 2]));
        assert_eq!(moved(4, 2, 1), Some(vec![0, 2, 1, 3]));
    }

    #[test]
    fn dropping_a_step_where_it_already_is_does_nothing() {
        assert_eq!(moved(4, 1, 1), None);
        assert_eq!(moved(4, 1, 2), None, "just below itself");
        assert_eq!(moved(4, 4, 0), None, "no such step");
        assert_eq!(moved(4, 0, 5), None, "no such place");
    }

    #[test]
    fn deleting_keeps_the_others_in_order() {
        let ticked: BTreeSet<usize> = [1, 3].into();
        assert_eq!(without(5, &ticked), Some(vec![0, 2, 4]));
        assert_eq!(without(5, &BTreeSet::new()), None);
        assert_eq!(without(2, &[7].into()), None, "positions out of range are ignored");
        assert_eq!(without(2, &[0, 1].into()), Some(vec![]));
    }
}
