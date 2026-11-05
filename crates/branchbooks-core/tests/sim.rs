//! reference/sim4.py and sim5.py against
//! `BudgetState`.

use branchbooks_core::budget::{BudgetState, Spend, TxEffect};
use branchbooks_core::money::{Decimal, ZERO, plain};

const CARD: &str = "Liabilities:CreditCard";

fn d(x: i64) -> Decimal {
    Decimal::from(x)
}

/// Exact decimal equality, with readable failures.
#[track_caller]
fn eq(actual: Decimal, expected: i64) {
    assert_eq!(plain(actual), expected.to_string());
}

#[derive(Clone)]
struct Sim {
    s: BudgetState,
}

impl Sim {
    fn new() -> Self {
        Sim {
            s: BudgetState::new("2026-10".parse().unwrap()),
        }
    }
    fn tx(&mut self, cash: i64, card: i64, spend: Option<(&str, i64, bool)>, payment: i64) {
        let card_delta = if card != 0 {
            vec![(CARD.to_owned(), d(card))]
        } else {
            vec![]
        };
        let spends = spend
            .map(|(n, x, on_card)| Spend {
                category: n.to_owned(),
                amount: d(x),
                card: on_card.then(|| CARD.to_owned()),
            })
            .into_iter()
            .collect();
        let payments = if payment != 0 {
            vec![(CARD.to_owned(), d(payment))]
        } else {
            vec![]
        };
        self.s.apply_tx(
            &TxEffect {
                cash: d(cash),
                card_delta,
                spends,
                payments,
                flags: vec![],
            },
            "2026-10-01".parse().unwrap(),
        );
    }
    fn cash(&self) -> Decimal {
        self.s.cash
    }
    fn income(&mut self, x: i64) {
        self.tx(x, 0, None, 0);
    }
    fn assign(&mut self, n: &str, x: i64) {
        self.s.assign(n, d(x), None);
    }
    fn move_(&mut self, a: &str, b: &str, x: i64) {
        self.s.move_(a, b, d(x), None);
    }
    fn spend_cash(&mut self, n: &str, x: i64) {
        self.tx(-x, 0, Some((n, x, false)), 0);
    }
    fn spend_card(&mut self, n: &str, x: i64) {
        self.tx(0, -x, Some((n, x, true)), 0);
    }
    fn refund_card(&mut self, n: &str, x: i64) {
        self.spend_card(n, -x);
    }
    fn pay_card(&mut self, x: i64) {
        self.tx(-x, x, None, x);
    }
    fn avail(&self, n: &str) -> Decimal {
        self.s.avail(n)
    }
    fn split(&self, n: &str) -> [String; 2] {
        let s = self.s.split(n);
        [plain(s.cash), plain(s.credit)]
    }
    fn pay(&self) -> Decimal {
        self.s.pay().get(CARD).copied().unwrap_or(ZERO)
    }
    fn owed(&self) -> Decimal {
        self.s.owed.get(CARD).copied().unwrap_or(ZERO)
    }
    fn unassigned(&self) -> Decimal {
        self.s.unassigned(ZERO)
    }
    fn unassigned_future(&self, future: i64) -> Decimal {
        self.s.unassigned(d(future))
    }
    fn rollover(&mut self) {
        self.s.rollover();
    }
}

fn split(cash: &str, credit: &str) -> [String; 2] {
    [cash.to_owned(), credit.to_owned()]
}

fn base() -> Sim {
    let mut b = Sim::new();
    b.income(3900);
    eq(b.unassigned(), 3900);
    for (n, x) in [
        ("Rent", 1500),
        ("Groceries", 600),
        ("Dining", 200),
        ("Investing", 0),
    ] {
        b.assign(n, x);
    }
    eq(b.unassigned(), 1600);
    b.spend_cash("Groceries", 120);
    eq(b.unassigned(), 1600);
    eq(b.avail("Groceries"), 480);
    b.spend_card("Dining", 45);
    eq(b.unassigned(), 1600);
    eq(b.pay(), 45);
    eq(b.avail("Dining"), 155);
    b.pay_card(45);
    eq(b.unassigned(), 1600);
    eq(b.pay(), 0);
    eq(b.cash(), 3735);
    b.spend_cash("Groceries", 500);
    eq(b.avail("Groceries"), -20);
    eq(b.cash(), 3235);
    eq(b.unassigned(), 1600);
    assert_eq!(b.split("Groceries"), split("20", "0"));
    let mut fixed = b.clone();
    fixed.assign("Groceries", 20);
    eq(fixed.avail("Groceries"), 0);
    eq(fixed.unassigned(), 1580);
    b.rollover();
    eq(b.avail("Groceries"), -20);
    eq(b.unassigned(), 1600);
    b.assign("Investing", 500);
    eq(b.unassigned(), 1100);
    b.spend_cash("Investing", 500); // transfer to brokerage with category
    eq(b.unassigned(), 1100);
    eq(b.cash(), 2735);
    eq(b.avail("Investing"), 0);
    b
}

#[test]
fn examples_1_to_5_build_the_base_state() {
    base();
}

#[test]
fn ex6_credit_overspending_only() {
    let mut a = base();
    a.spend_card("Dining", 200);
    eq(a.avail("Dining"), -45);
    assert_eq!(a.split("Dining"), split("0", "45"));
    eq(a.pay(), 155);
    eq(a.owed(), 200);
    eq(a.unassigned(), 1100);
    let mut cov = a.clone();
    cov.assign("Dining", 45);
    eq(cov.avail("Dining"), 0);
    eq(cov.pay(), 200);
    eq(cov.unassigned(), 1055);
    let mut unc = a.clone();
    unc.rollover();
    eq(unc.avail("Dining"), 0);
    eq(unc.pay(), 155);
    eq(unc.owed() - unc.pay(), 45);
    eq(unc.unassigned(), 1100);
}

#[test]
fn ex7_refund() {
    let mut r = base();
    r.spend_card("Dining", 60);
    r.refund_card("Dining", 25);
    eq(r.avail("Dining"), 120);
    eq(r.pay(), 35);
    eq(r.owed(), 35);
    eq(r.unassigned(), 1100);
}

#[test]
fn ex8_future_month() {
    let f = base();
    eq(f.unassigned_future(300), 800);
    eq(f.avail("Rent"), 1500);
}

#[test]
fn ex9_mixed_overspending_corrections_cover_cash_first() {
    let mut m = base();
    m.spend_card("Dining", 200);
    m.spend_cash("Dining", 30);
    eq(m.avail("Dining"), -75);
    assert_eq!(m.split("Dining"), split("30", "45"));
    eq(m.cash(), 2705);
    eq(m.pay(), 155);
    eq(m.owed(), 200);
    eq(m.unassigned(), 1100);

    let mut p20 = m.clone();
    p20.assign("Dining", 20);
    eq(p20.avail("Dining"), -55);
    assert_eq!(p20.split("Dining"), split("10", "45"));
    eq(p20.pay(), 155);
    eq(p20.unassigned(), 1080);

    let mut p40 = m.clone();
    p40.assign("Dining", 40);
    eq(p40.avail("Dining"), -35);
    assert_eq!(p40.split("Dining"), split("0", "35"));
    eq(p40.pay(), 165);
    eq(p40.unassigned(), 1060);
    p40.rollover();
    eq(p40.avail("Dining"), 0);
    eq(p40.pay(), 165);
    eq(p40.owed() - p40.pay(), 35);
    eq(p40.unassigned(), 1060);

    let mut p75 = m.clone();
    p75.assign("Dining", 75);
    eq(p75.avail("Dining"), 0);
    eq(p75.pay(), 200);
    eq(p75.unassigned(), 1025);

    let mut none = m.clone();
    none.rollover();
    eq(none.avail("Dining"), -30);
    eq(none.pay(), 155);
    eq(none.owed() - none.pay(), 45);
    eq(none.unassigned(), 1100);

    let mut pre = base();
    pre.assign("Dining", 40);
    pre.spend_card("Dining", 200);
    pre.spend_cash("Dining", 30);
    eq(pre.avail("Dining"), -35);
    assert_eq!(pre.split("Dining"), split("30", "5"));
    eq(pre.pay(), 195);
    eq(pre.unassigned(), 1060);
}

#[test]
fn purchase_order_does_not_matter() {
    let run = |cash_first: bool| {
        let mut t = Sim::new();
        t.income(5000);
        t.assign("Dining", 100);
        for cash in [cash_first, !cash_first] {
            if cash {
                t.spend_cash("Dining", 60);
            } else {
                t.spend_card("Dining", 70);
            }
        }
        (t.split("Dining"), plain(t.pay()), plain(t.unassigned()))
    };
    assert_eq!(run(true), run(false));
    assert_eq!(
        run(true),
        (split("30", "0"), "70".to_owned(), "4900".to_owned())
    );
}

#[test]
fn ex10_and_11() {
    let mut s = base();
    s.spend_cash("Groceries", -150);
    s.income(50);
    eq(s.cash(), 2935);
    eq(s.avail("Groceries"), 130);
    eq(s.unassigned(), 1150);
    let mut mv = base();
    mv.move_("Rent", "Groceries", 20);
    eq(mv.avail("Rent"), 1480);
    eq(mv.avail("Groceries"), 0);
    eq(mv.unassigned(), 1100);
    assert_eq!(mv.split("Groceries"), split("0", "0"));
}
