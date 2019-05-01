use proptest::collection;
use proptest::prelude::*;

use arenavec::{Arena, SliceVec};

const DEFAULT_CAPACITY: usize = 4096 << 16;

mod isolated {
    use super::*;

    #[test]
    fn init_empty() {
        let arena = Arena::init_capacity(DEFAULT_CAPACITY);

        let vec: SliceVec<usize> = SliceVec::new(arena.inner(), 0);

        assert_eq!(vec.len(), 0);
        assert_eq!(vec.capacity(), 0);
    }

    #[test]
    fn init_capacity() {
        let arena = Arena::init_capacity(DEFAULT_CAPACITY);

        let mut vec = SliceVec::new(arena.inner(), 10);

        assert_eq!(vec.len(), 0);
        assert_eq!(vec.capacity(), 10);

        for i in 0..10 {
            vec.push(i);
        }

        assert_eq!(vec.len(), 10);
        assert_eq!(vec.capacity(), 10);
    }

    #[test]
    fn init_empty_push() {
        let arena = Arena::init_capacity(DEFAULT_CAPACITY);

        let mut vec = SliceVec::new(arena.inner(), 0);

        assert_eq!(vec.len(), 0);
        assert_eq!(vec.capacity(), 0);

        vec.push(1);

        assert_eq!(vec.len(), 1);
        assert_eq!(vec.capacity(), 4);

        vec.push(2);
        vec.push(3);
        vec.push(4);

        assert_eq!(vec.len(), 4);
        assert_eq!(vec.capacity(), 4);

        vec.push(5);

        assert_eq!(vec.len(), 5);
        assert_eq!(vec.capacity(), 8);
    }

    #[test]
    fn reserve_and_resize() {
        let arena = Arena::init_capacity(DEFAULT_CAPACITY);

        let mut vec = SliceVec::new(arena.inner(), 0);

        assert_eq!(vec.len(), 0);
        assert_eq!(vec.capacity(), 0);

        for i in 0..5 {
            vec.push(i);
        }

        assert_eq!(vec.len(), 5);
        assert_eq!(vec.capacity(), 8);

        vec.reserve(6);

        assert_eq!(vec.capacity(), 8);

        vec.reserve(9);

        assert_eq!(vec.capacity(), 16);

        vec.resize(12, 1);

        assert_eq!(vec.len(), 12);
        assert_eq!(vec.capacity(), 16);

        for i in 5..12 {
            assert_eq!(vec[i], 1);
        }
    }
}

mod rand_static {
    use super::*;

    #[derive(Clone, Debug)]
    enum SliceVecOp {
        Push(usize),
        Resize(usize, usize),
        Reserve(usize),
    }

    prop_compose! {
        fn arb_op() (id in 0..3, size in 0..100, val in 0..std::usize::MAX) -> SliceVecOp {
            match id {
                 0 => SliceVecOp::Push(val),
                 1 => SliceVecOp::Resize(size as usize, val),
                 2 => SliceVecOp::Reserve(size as usize),
                 _ => unreachable!(),
            }
        }
    }

    prop_compose! {
        fn arb_op_seq(num_vecs: usize, len: usize)
            (ids in collection::vec(0..num_vecs, len),
             ops in collection::vec(arb_op(), len))
        -> Vec<(usize, SliceVecOp)> {
            let mut res = Vec::with_capacity(len);

            for i in 0..len {
                res.push((ids[i], ops[i].clone()));
            }

            res
        }
    }

    const NUM_VECS: usize = 8;
    const NUM_OPS: usize = 400;

    proptest! {
        #[test]
        fn rand_op_sec(mut seq in arb_op_seq(NUM_VECS, NUM_OPS)) {
            let arena = Arena::init_capacity(DEFAULT_CAPACITY);
            let mut vecs = Vec::with_capacity(NUM_VECS);
            let mut slice_vecs = Vec::with_capacity(NUM_VECS);

            for _ in 0..NUM_VECS {
                vecs.push(Vec::new());
            }

            for _ in 0..NUM_VECS {
                slice_vecs.push(SliceVec::new(arena.inner(), 0));
            }

            for (v, op) in seq.drain(..) {
                match op {
                    SliceVecOp::Push(e) => {
                        vecs[v].push(e);
                        slice_vecs[v].push(e);
                    },
                    SliceVecOp::Resize(l, e) => {
                        vecs[v].resize(l, e);
                        slice_vecs[v].resize(l, e);
                    },
                    SliceVecOp::Reserve(l) => {
                        vecs[v].reserve(l);
                        slice_vecs[v].reserve(l);
                    },
                }

                for i in 0..NUM_VECS {
                    assert_eq!(&*vecs[i], &*slice_vecs[i]);
                }
            }
        }
    }
}

mod rand_dynamic {
    use super::*;

    #[derive(Clone, Debug)]
    enum SliceVecOp {
        Push(usize),
        Resize(usize, usize),
        Reserve(usize),
        Delete,
        Clone(usize),
    }

    prop_compose! {
        fn arb_op(num_vecs: usize)
            (id in 0..5,
             size in 0..100,
             val in 0..std::usize::MAX,
             index in 0..num_vecs)
            -> SliceVecOp
        {
            match id {
                 0 => SliceVecOp::Push(val),
                 1 => SliceVecOp::Resize(size as usize, val),
                 2 => SliceVecOp::Reserve(size as usize),
                 3 => SliceVecOp::Delete,
                 4 => SliceVecOp::Clone(index),
                 _ => unreachable!(),
            }
        }
    }

    prop_compose! {
        fn arb_op_seq(num_vecs: usize, len: usize)
            (ids in collection::vec(0..num_vecs, len),
             ops in collection::vec(arb_op(num_vecs), len))
        -> Vec<(usize, SliceVecOp)> {
            let mut res = Vec::with_capacity(len);

            for i in 0..len {
                res.push((ids[i], ops[i].clone()));
            }

            res
        }
    }

    const NUM_VECS: usize = 16;
    const NUM_OPS: usize = 800;

    proptest! {
        #[test]
        fn rand_op_sec(mut seq in arb_op_seq(NUM_VECS, NUM_OPS)) {
            let arena = Arena::init_capacity(DEFAULT_CAPACITY);
            let mut vecs: Vec<Option<Vec<usize>>> = vec![None; NUM_VECS];
            let mut slice_vecs: Vec<Option<SliceVec<usize>>> = vec![None; NUM_VECS];

            for (v, op) in seq.drain(..) {
                match op {
                    SliceVecOp::Push(e) => {
                        if let (&mut Some(ref mut r), &mut Some(ref mut r2)) =
                            (&mut vecs[v], &mut slice_vecs[v])
                        {
                            r.push(e);
                            r2.push(e);
                        } else {
                            let mut vec = Vec::new();
                            let mut slice_vec = SliceVec::new(arena.inner(), 0);

                            vec.push(e);
                            slice_vec.push(e);

                            vecs[v] = Some(vec);
                            slice_vecs[v] = Some(slice_vec);
                        }
                    },
                    SliceVecOp::Resize(l, e) => {
                        if let (&mut Some(ref mut r), &mut Some(ref mut r2)) =
                            (&mut vecs[v], &mut slice_vecs[v])
                        {
                            r.resize(l, e);
                            r2.resize(l, e);
                        } else {
                            let mut vec = Vec::new();
                            let mut slice_vec = SliceVec::new(arena.inner(), 0);

                            vec.resize(l, e);
                            slice_vec.resize(l, e);

                            vecs[v] = Some(vec);
                            slice_vecs[v] = Some(slice_vec);
                        }
                    },
                    SliceVecOp::Reserve(l) => {
                        if let (&mut Some(ref mut r), &mut Some(ref mut r2)) =
                            (&mut vecs[v], &mut slice_vecs[v])
                        {
                            r.reserve(l);
                            r2.reserve(l);
                        } else {
                            let mut vec = Vec::new();
                            let mut slice_vec = SliceVec::new(arena.inner(), 0);

                            vec.reserve(l);
                            slice_vec.reserve(l);

                            vecs[v] = Some(vec);
                            slice_vecs[v] = Some(slice_vec);
                        }
                    },
                    SliceVecOp::Delete => {
                        vecs[v] = None;
                        slice_vecs[v] = None;
                    },
                    SliceVecOp::Clone(i) => {
                        vecs[v] = vecs[i].clone();
                        slice_vecs[v] = slice_vecs[i].clone();
                    },
                }

                for i in 0..NUM_VECS {
                    if let (&Some(ref r), &Some(ref r2)) = (&vecs[i], &slice_vecs[i]) {
                        assert_eq!(&**r, &**r2);
                    } else if vecs[i].is_some() || slice_vecs[i].is_some() {
                        panic!("missing vec");
                    }
                }
            }
        }
    }
}
