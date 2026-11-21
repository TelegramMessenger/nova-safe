// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Every account taken, and one given up while a block of it lives on. Its
//! own test binary: it takes the whole table, which no other test may use
//! meanwhile.

use core::hint::black_box;

use nova_meter::{Account, Metered};

#[global_allocator]
static METERED: Metered = Metered;

const HEADER: usize = 16;

/// 1023 accounts are counted (the table's first is the uncounted one);
/// with all of them taken a new account is the uncounted one, which reads
/// nothing and takes allocations all the same. An account given up while a
/// block made on it lives goes to the next engine with that block on it,
/// which leaves when it is freed; given up again, the account is counted
/// for the one after.
#[test]
fn accounts_run_out_and_come_back() {
    let mut taken = Vec::with_capacity(1024);
    for _ in 0..1023 {
        let account = Account::new();
        assert!(
            account.bytes().is_some(),
            "account {} is counted",
            taken.len() + 1
        );
        taken.push(account);
    }
    let none = Account::new();
    assert_eq!(none.bytes(), None, "every account is taken");
    {
        let _entered = none.enter();
        let block = black_box(vec![1u8; 4096]);
        assert_eq!(none.bytes(), None);
        drop(block);
    }
    drop(none);
    assert_eq!(
        Account::new().bytes(),
        None,
        "giving up the uncounted account frees nothing"
    );

    let first = taken.swap_remove(0);
    let before = first.bytes().expect("counted");
    let left_over = {
        let _entered = first.enter();
        black_box(vec![2u8; 1000])
    };
    assert_eq!(first.bytes(), Some(before + 1000 + HEADER));
    drop(first);
    let next = Account::new();
    assert_eq!(
        next.bytes(),
        Some(before + 1000 + HEADER),
        "the one free account, with the block on it"
    );
    let own = {
        let _entered = next.enter();
        black_box(vec![3u8; 500])
    };
    assert_eq!(next.bytes(), Some(before + 1000 + 500 + 2 * HEADER));
    drop(left_over);
    assert_eq!(next.bytes(), Some(before + 500 + HEADER));
    drop(own);
    assert_eq!(next.bytes(), Some(before));
    assert_eq!(Account::new().bytes(), None, "all taken again");

    drop(next);
    let again = Account::new();
    assert_eq!(again.bytes(), Some(before));
    drop(again);
    drop(taken);
    let counted: Vec<Account> = (0..1023).map(|_| Account::new()).collect();
    assert!(
        counted.iter().all(|account| account.bytes().is_some()),
        "all free again"
    );
}
