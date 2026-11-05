# Budget rules

The rules the budget follows, and the worked examples the code must match. Tests hold the code to
them: `crates/branchbooks-core/tests/`. The reasoning behind the less obvious ones is in
[DESIGN.md](DESIGN.md).

## Flags

Budget flags are metadata on `open` directives:

- `budget: "on"` or `budget: "off"` on asset and liability accounts.
- `budget: "excluded"` on income and expense accounts that never touch an envelope. A category with
  no flag is budgeted.

## Rules

Terms: Available is an envelope's balance. Unassigned is money not yet assigned. Activity is spending
(negative) or refunds (positive).

1. Available in month M = prior month's Available + assigned in M + moved in M + activity in M. The prior Available is negative only when it carries a cash overspend (rule 6).
2. Unassigned = on-budget cash account balances minus the sum of every envelope's Available (including card payment envelopes) minus money assigned to future months. Credit card balances are not subtracted. The credit-overspent part of a category counts as zero.
3. Income: money arriving in an on-budget account from a non-budget source raises Unassigned by exactly the on-budget posting. Excluded postings in the same transaction have no effect. A deposit that is partly a reimbursement uses an income account (such as `Income:Other`) for the non-reimbursement part.
4. Card spending: a purchase reduces the category's Available. Only the funded part (up to the Available the category had) moves into the card payment envelope. Paying the card reduces the payment envelope, whatever else the transaction holds (a fee, say). A balance transfer moves the same amount from the paid card's payment envelope to the charged card's, and Unassigned does not change.
5. Overspending is split into a cash portion and a credit portion. Cash portion = min(overspend, carried-in cash overspend + cash spent this month − cash already corrected). The rest is the credit portion. This depends only on month totals, never on purchase order. An assignment or move into a category that is currently overspent is a correction: it covers the cash portion first, then the credit portion. Money assigned before the overspend simply counts as funds.
6. Cash overspending not corrected by month end carries into the next month as a negative Available. Unassigned does not change. (A design choice. The alternative is to subtract it from next month's Unassigned.)
7. Credit overspending: the credit portion never reaches the payment envelope, so the card owes more than is set aside and is flagged underfunded. Unassigned does not change. At month end the credit portion is dropped from the category.
8. Off-budget transfers: moving money from an on-budget to an off-budget account must carry a `category:` metadata line naming a budgeted category. Without one, the transaction is flagged as uncategorized. Only the off-budget amount that on-budget money paid for counts (what left cash and cards, less what the same transaction spent in categories), and only it is charged to the category. Money that reaches an off-budget account from income, such as a 401k contribution in a paycheck, is not a transfer.
9. Refunds are an inflow to the original category. Its Available rises, the payment envelope falls by the same amount, and Unassigned does not change.
10. Future months: an assignment dated in a future month leaves Unassigned immediately.
11. `envelope-move` shifts Available between envelopes and never changes Unassigned.
12. Unspent positive Available always carries forward.
13. One budget currency. Accounts in other currencies are off-budget.
14. Same-day order: within a date, apply transactions first, then `envelope-assign`, then `envelope-move`. Entries of the same kind on the same date apply in file order. This affects only the budget calculation, not any other processing.
15. Entry dates: an `envelope-assign` or `envelope-move` can be dated any day, and its month is the calendar month of its date. A warning is emitted when an assignment or move acts as a correction because it is dated after that month's spending in the same category. The warning never changes the numbers.

Directive syntax:

```
2026-10-01 custom "envelope-assign" Expenses:Food:Groceries 600.00 USD
2026-10-10 custom "envelope-move" Expenses:Housing:Rent Expenses:Food:Groceries 20.00 USD
```

## Target budgeting

Targets are rules; envelopes are their results. A target is a `custom "target"` entry, so files stay
valid Beancount:

```
2026-01-01 custom "target" Expenses:Housing:Rent 1400.00 USD
  recurrence: "monthly"
  rollover: "reset"
2026-01-01 custom "target" Expenses:Savings:CarFund 6000.00 USD
  due: 2026-06-01
```

The allocations a target implies are virtual: computed, never written to the ledger. A savings goal is
a virtual envelope: a category such as `Expenses:Savings:CarFund`, while the money stays in the account
it is in.

- **T1** One target per category is in force. A target entry applies from the month of its date until a later target entry for the same category replaces it. An amount of 0 ends the target. A change is a new dated entry, so the rules in force in past months never change: merging a scenario adopts its rules from their dates on.
- **T2** With `due:` the target is one-time (`once`). Otherwise it is `monthly`. `rollover` is `carry-shortfall` (the default for one-time targets) or `reset` (the default for monthly ones). `reset` needs a monthly target.
- **T3** The allocation a target needs in month M. Here "put in" means carried in plus assigned plus moved, before the month's spending.
  - Monthly with `reset`: refill up to the amount, `max(0, amount − put in)`.
  - Monthly with `carry-shortfall`: the amount plus what earlier months failed to fund, less what was assigned or moved in M.
  - One-time: `(amount − carried in) ÷ months left including M`, rounded down to the amount's decimals, less what was assigned or moved in M. The due month takes the remainder. Nothing is allocated after the due month or once the target is met.
- **T4** Mixed budgets: a category without a target is budgeted by `envelope-assign` alone. A category with a target still takes assignments and moves, and they count toward it through "put in".
- **T5** Targets are funded at the end of each month, after the month's own entries. They are funded in order of due month (monthly targets are due this month), then category name. Funding draws only on the money on hand that month. Money assigned to later months is not counted, because a month must depend only on itself and earlier months. What cannot be funded is the target's shortfall for the month, reported as a shortfall alert. Funding covers a cash overspend first, as an assignment does (rule 5).
- **T6** Unassigned "now" includes target allocations up to today's month. Later months are projections. Since funding ignores later assignments (T5), targets plus a future-dated assignment can leave Unassigned negative, which shows the over-commitment.
- **T7** A target on something that is not a budgeted category, or with bad fields, is flagged (`envelope`) and changes nothing. The write path refuses it.

## Reconcile

- **R1** A transaction flagged `*` is cleared; one flagged `!` is pending (Beancount's own flags).
- **R2** `reconcile(account, statement date, statement balance)` compares the statement with the cleared balance on that date. It returns the difference and the pending transactions that could explain it.
- **R3** When they match, one commit (Op-Type `reconcile`) writes a Beancount `balance` assertion dated the day after the statement, since Beancount checks assertions at the start of the day. The same commit flips the matched pending transactions to `*`.
- **R4** An assertion that later fails is a ledger error, like an unbalanced transaction, and the write path refuses changes that would add one.

## Worked examples

`fixtures/examples-1-5.bean` holds Examples 1 to 5, and every value below is checked in
`crates/branchbooks-core/tests/`. `reference/sim4.py` is a Python model of the rules, and
`reference/sim5.py` adds the same-day ordering tests; `crates/branchbooks-core/tests/sim.rs` holds the
core to their assertions. Run them with `python3 -I`.

State after Example 5 (the base state for Examples 6 to 12): Checking 2,735.00, Rent 1,500.00, Groceries
-20.00 (carried cash overspend), Dining 155.00, Unassigned 1,100.00.

| Example | Event | Expected |
|---|---|---|
| 1 | Paycheck | Checking 3,900.00, Unassigned 3,900.00. Income statement: income 5,000.00, expenses 1,100.00, net 3,900.00 |
| 2 | Assign Rent 1,500, Groceries 600, Dining 200 | Unassigned 1,600.00 |
| 3 | Groceries 120 (cash), Dining 45 (card), pay card 45 | After groceries: Groceries 480. After card: Dining 155, card owed 45, payment envelope 45. After payment: Checking 3,735.00, payment envelope 0. Unassigned 1,600.00 throughout |
| 4 | Groceries 500 cash on Oct 30 | Groceries -20.00, Checking 3,235.00, Unassigned 1,600.00. Uncorrected on Nov 1: Groceries -20.00, Unassigned 1,600.00. If 20.00 is assigned before Nov 1: Groceries 0.00, Unassigned 1,580.00 |
| 5 | Assign Investing 500, transfer to brokerage with `category` | Unassigned 1,600 then 1,100 after the assignment, and 1,100 after the transfer. Investing 0.00, Checking 2,735.00, brokerage 500.00. Without the `category` line: flagged uncategorized |
| 6 | Nov 5 card purchase, Dining 200 | Dining -45.00 (all credit), payment envelope 155.00, card owes 200.00, Unassigned 1,100.00. Covered with 45: Dining 0, payment envelope 200, Unassigned 1,055. Uncovered at Dec 1: Dining 0, payment envelope 155, card underfunded by 45, Unassigned 1,100 |
| 7 | Nov 8 card purchase Dining 60, Nov 12 refund 25 | After purchase: Dining 95, payment envelope 60. After refund: Dining 120, payment envelope 35, card owes 35. Unassigned 1,100.00 throughout |
| 8 | Assign 300 to Rent dated 2026-12-01, entered Nov 3 | Unassigned 800.00. November Rent 1,500.00. December Rent starts at 1,800.00 |
| 9 | After Example 6, Nov 6 cash purchase Dining 30 | Dining -75.00 (30 cash, 45 credit), Checking 2,705.00, payment envelope 155.00, Unassigned 1,100.00. Assign 20: 10 cash, 45 credit, Unassigned 1,080. Assign 40: 0 cash, 35 credit, payment envelope 165, Unassigned 1,060. Assign 75: Dining 0, payment envelope 200, Unassigned 1,025. Nothing corrected at Dec 1: Dining -30 (cash carries), card underfunded by 45, Unassigned 1,100. The same 40 assigned on Nov 1 before the purchases: 30 cash, 5 credit |
| 10 | Nov 4 deposit: Checking +200, Groceries -150, Income:Other -50 | Checking 2,935.00, Groceries 130.00, Unassigned 1,150.00 |
| 11 | Nov 10 move 20 from Rent to Groceries | Rent 1,480.00, Groceries 0.00, Unassigned 1,100.00 |
| 12 | Nov 6: assign Dining 40 and cash purchase Dining 30, after the Nov 5 credit overspend | Same result in either file order: Dining -35.00 (all credit), payment envelope 165.00, Unassigned 1,060.00 |
| 14 | Reports on the Examples 1 to 5 ledger as of Nov 2 | Income 5,000.00. Expenses 1,765.00 (tax 800, 401k 300, groceries 620, dining 45), net 3,235.00. Assets 3,235.00 (checking 2,735, brokerage 500), liabilities 0.00, net worth 3,235.00. October budget vs actual: Rent assigned 1,500 / activity 0 / Available 1,500. Groceries 600 / -620 / -20. Dining 200 / -45 / 155 |

Purchase order does not matter: from 100.00 Available, a 60.00 cash purchase and a 70.00 card purchase
leave 30.00 of cash overspending in either order.
