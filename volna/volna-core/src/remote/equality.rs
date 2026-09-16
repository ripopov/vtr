//! Compare repeated raw relations without serializing their attributes again.
//! Byte comparisons and traversal yield, including within a single large value.
use crate::data::loaded_tracks::LoadedRelation;
use crate::data::transactions::{AttributeValue, Attributes};

enum Work<'a> {
    Bytes(&'a [u8], &'a [u8]),
    Attributes(
        &'a [(String, AttributeValue)],
        &'a [(String, AttributeValue)],
    ),
    Values(&'a [AttributeValue], &'a [AttributeValue]),
    Value(&'a AttributeValue, &'a AttributeValue),
}

pub(super) async fn relation<F: std::future::Future<Output = ()>>(
    a: &LoadedRelation,
    b: &LoadedRelation,
    checkpoint: &mut impl FnMut() -> F,
) -> bool {
    if a.id != b.id
        || a.from_generator != b.from_generator
        || a.to_generator != b.to_generator
        || a.relation.from != b.relation.from
        || a.relation.to != b.relation.to
    {
        return false;
    }
    let mut work = vec![
        Work::Bytes(a.relation.kind.as_bytes(), b.relation.kind.as_bytes()),
        attrs(&a.relation.attributes, &b.relation.attributes),
    ];
    while let Some(item) = work.pop() {
        checkpoint().await;
        match item {
            Work::Bytes(a, b) => {
                if a.len() != b.len() {
                    return false;
                }
                let count = a.len().min(4096);
                if a[..count] != b[..count] {
                    return false;
                }
                if count < a.len() {
                    work.push(Work::Bytes(&a[count..], &b[count..]));
                }
            }
            Work::Attributes(a, b) => {
                if a.len() != b.len() {
                    return false;
                }
                if let (Some((first_a, tail_a)), Some((first_b, tail_b))) =
                    (a.split_first(), b.split_first())
                {
                    work.push(Work::Attributes(tail_a, tail_b));
                    work.push(Work::Value(&first_a.1, &first_b.1));
                    work.push(Work::Bytes(first_a.0.as_bytes(), first_b.0.as_bytes()));
                }
            }
            Work::Values(a, b) => {
                if a.len() != b.len() {
                    return false;
                }
                if let (Some((first_a, tail_a)), Some((first_b, tail_b))) =
                    (a.split_first(), b.split_first())
                {
                    work.push(Work::Values(tail_a, tail_b));
                    work.push(Work::Value(first_a, first_b));
                }
            }
            Work::Value(a, b) => {
                use AttributeValue::*;
                match (a, b) {
                    (F64(a), F64(b)) => {
                        if a.to_bits() != b.to_bits() {
                            return false;
                        }
                    }
                    (Text(a), Text(b)) => work.push(Work::Bytes(a.as_bytes(), b.as_bytes())),
                    (Bytes(a), Bytes(b)) => work.push(Work::Bytes(a, b)),
                    (
                        Logic {
                            width: a_width,
                            states: a_states,
                            data: a,
                        },
                        Logic {
                            width: b_width,
                            states: b_states,
                            data: b,
                        },
                    ) => {
                        if a_width != b_width || a_states != b_states {
                            return false;
                        }
                        work.push(Work::Bytes(a, b));
                    }
                    (
                        Enum {
                            value: a_value,
                            name: a,
                        },
                        Enum {
                            value: b_value,
                            name: b,
                        },
                    ) => {
                        if a_value != b_value {
                            return false;
                        }
                        work.push(Work::Bytes(a.as_bytes(), b.as_bytes()));
                    }
                    (List(a), List(b)) => work.push(Work::Values(a, b)),
                    (Map(a), Map(b)) => work.push(attrs(a, b)),
                    _ => {
                        if a != b {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

fn attrs<'a>(a: &'a Attributes, b: &'a Attributes) -> Work<'a> {
    Work::Attributes(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::transactions::{Relation, TrackRef, TransactionRef};
    use std::cell::Cell;
    use std::future::{Future, poll_fn};
    use std::task::{Context, Poll, Waker};

    fn compare(a: &LoadedRelation, b: &LoadedRelation) -> (bool, usize) {
        let credits = Cell::new(0);
        let mut checkpoint = || {
            poll_fn(|_| {
                if credits.get() == 0 {
                    Poll::Pending
                } else {
                    credits.set(credits.get() - 1);
                    Poll::Ready(())
                }
            })
        };
        let mut future = std::pin::pin!(relation(a, b, &mut checkpoint));
        let mut polls = 0;
        loop {
            polls += 1;
            credits.set(8);
            match future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
            {
                Poll::Pending => assert_eq!(credits.get(), 0),
                Poll::Ready(equal) => return (equal, polls),
            }
        }
    }

    #[test]
    fn nested_attributes_compare_bitwise_and_large_values_yield() {
        let a = LoadedRelation {
            id: 1,
            from_generator: TrackRef(1),
            to_generator: TrackRef(2),
            relation: Relation {
                kind: "raw".into(),
                from: TransactionRef(1),
                to: TransactionRef(2),
                attributes: vec![(
                    "values".into(),
                    AttributeValue::List(vec![
                        AttributeValue::Map(vec![("nan".into(), AttributeValue::F64(f64::NAN))]),
                        AttributeValue::Text("λ".repeat(256 * 1024)),
                    ]),
                )],
            },
        };
        let mut b = a.clone();
        let (equal, polls) = compare(&a, &b);
        assert!(equal && polls > 8);
        let AttributeValue::List(values) = &mut b.relation.attributes[0].1 else {
            unreachable!()
        };
        let AttributeValue::Text(text) = &mut values[1] else {
            unreachable!()
        };
        text.push('x');
        assert!(!compare(&a, &b).0);
        let mut a = a;
        a.relation.attributes = vec![("zero".into(), AttributeValue::F64(-0.0))];
        b.relation.attributes = vec![("zero".into(), AttributeValue::F64(0.0))];
        assert!(!compare(&a, &b).0);
    }
}
