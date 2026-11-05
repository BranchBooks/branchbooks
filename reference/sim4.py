"""Envelope budget simulation, v4: corrections cover the cash portion before the credit portion.

Attribution of an overspend (cash-first, by month totals of spending):
    cash_claims = carried cash overspend + cash spent this month - cash already corrected
    cash_over   = min(overspend, cash_claims)
    credit_over = overspend - cash_over
Corrections: an assignment or move INTO a category that is currently overspent covers the
cash portion first (recorded in `cured`), then the credit portion. Money assigned while the
category is not overspent is simply funds.
"""
from decimal import Decimal as D
import copy


class Cat:
    def __init__(self):
        self.A0 = D(0)     # opening Available (negative only if a cash overspend was carried)
        self.a = D(0)      # assigned + moved in/out this month
        self.c = D(0)      # cash spending this month (net of cash refunds)
        self.k = D(0)      # card spending this month (net of card refunds)
        self.cured = D(0)  # cash overspend corrected this month


class Budget:
    def __init__(self):
        self.cash = D(0)
        self.owed = D(0)
        self.pay_carried = D(0)
        self.payments = D(0)
        self.future = D(0)
        self.brokerage = D(0)
        self.cats = {}

    def cat(self, n):
        return self.cats.setdefault(n, Cat())

    def avail(self, n):
        c = self.cat(n)
        return c.A0 + c.a - c.c - c.k

    def split(self, n):
        c = self.cat(n)
        over = max(-self.avail(n), D(0))
        claims = max(max(-c.A0, D(0)) + c.c - c.cured, D(0))
        cash_over = min(over, claims)
        return cash_over, over - cash_over

    def pay(self):
        total = self.pay_carried - self.payments
        for n, c in self.cats.items():
            total += c.k - self.split(n)[1]
        return total

    def unassigned(self):
        env = D(0)
        for n in self.cats:
            env += self.avail(n) + self.split(n)[1]
        return self.cash - env - self.pay() - self.future

    # operations
    def assign(self, n, x):
        x = D(x)
        c = self.cat(n)
        if x > 0:
            c.cured += min(x, self.split(n)[0])   # correction covers the cash portion first
        c.a += x

    def spend_cash(self, n, x): self.cash -= D(x); self.cat(n).c += D(x)
    def spend_card(self, n, x): self.owed += D(x); self.cat(n).k += D(x)
    def pay_card(self, x): self.cash -= D(x); self.owed -= D(x); self.payments += D(x)
    def refund_card(self, n, x): self.owed -= D(x); self.cat(n).k -= D(x)

    def move(self, src, dst, x):
        self.cat(src).a -= D(x)
        self.assign(dst, x)

    def rollover(self):
        pay_now = self.pay()
        for n, c in self.cats.items():
            av = self.avail(n)
            cash_over, _ = self.split(n)
            c.A0 = av if av >= 0 else -cash_over
            c.a = c.c = c.k = c.cured = D(0)
        self.pay_carried = pay_now
        self.payments = D(0)

    def row(self, label):
        g = lambda n: self.avail(n) if n in self.cats else D(0)
        sp = self.split("Dining") if "Dining" in self.cats else (D(0), D(0))
        print(f"{label:30s} chk={self.cash:>6} owed={self.owed:>4} pay={self.pay():>4} "
              f"rent={g('Rent'):>5} groc={g('Groceries'):>5} dine={g('Dining'):>5} "
              f"(cash {sp[0]}/credit {sp[1]}) Unassigned={self.unassigned():>5}")


b = Budget()
assert sum([D(-5000), D(800), D(300), D(3900)]) == 0
b.cash += 3900
assert b.unassigned() == 3900
for n, x in [("Rent", 1500), ("Groceries", 600), ("Dining", 200), ("Investing", 0)]:
    b.assign(n, x)
assert b.unassigned() == 1600

b.spend_cash("Groceries", 120); assert b.unassigned() == 1600 and b.avail("Groceries") == 480
b.spend_card("Dining", 45); assert b.unassigned() == 1600 and b.pay() == 45 and b.avail("Dining") == 155
b.pay_card(45); assert b.unassigned() == 1600 and b.pay() == 0 and b.cash == 3735

b.spend_cash("Groceries", 500)
assert b.avail("Groceries") == -20 and b.cash == 3235 and b.unassigned() == 1600
assert b.split("Groceries") == (D(20), D(0))
fixed = copy.deepcopy(b); fixed.assign("Groceries", 20)
assert fixed.avail("Groceries") == 0 and fixed.unassigned() == 1580
b.rollover()
assert b.avail("Groceries") == -20 and b.unassigned() == 1600

b.assign("Investing", 500); assert b.unassigned() == 1100
b.cash -= 500; b.cat("Investing").c += 500; b.brokerage += 500
assert b.unassigned() == 1100 and b.cash == 2735 and b.avail("Investing") == 0
base = b

# Ex 6: credit overspending only
a = copy.deepcopy(base); a.spend_card("Dining", 200)
a.row("Ex6 Nov5 card 200")
assert a.avail("Dining") == -45 and a.split("Dining") == (D(0), D(45))
assert a.pay() == 155 and a.owed == 200 and a.unassigned() == 1100
cov = copy.deepcopy(a); cov.assign("Dining", 45)
assert cov.avail("Dining") == 0 and cov.pay() == 200 and cov.unassigned() == 1055
unc = copy.deepcopy(a); unc.rollover()
assert unc.avail("Dining") == 0 and unc.pay() == 155 and unc.owed - unc.pay() == 45 and unc.unassigned() == 1100

# Ex 7: refund
r = copy.deepcopy(base)
r.spend_card("Dining", 60); r.refund_card("Dining", 25)
assert r.avail("Dining") == 120 and r.pay() == 35 and r.owed == 35 and r.unassigned() == 1100

# Ex 8: future month
f = copy.deepcopy(base); f.future += 300
assert f.unassigned() == 800 and f.avail("Rent") == 1500

# Ex 9: mixed overspending, corrections cover cash first
m = copy.deepcopy(base)
m.spend_card("Dining", 200); m.spend_cash("Dining", 30)
m.row("Ex9 Nov6 mixed")
assert m.avail("Dining") == -75 and m.split("Dining") == (D(30), D(45))
assert m.cash == 2705 and m.pay() == 155 and m.owed == 200 and m.unassigned() == 1100

p20 = copy.deepcopy(m); p20.assign("Dining", 20)
p20.row("Ex9 assign 20")
assert p20.avail("Dining") == -55 and p20.split("Dining") == (D(10), D(45))
assert p20.pay() == 155 and p20.unassigned() == 1080

p40 = copy.deepcopy(m); p40.assign("Dining", 40)
p40.row("Ex9 assign 40")
assert p40.avail("Dining") == -35 and p40.split("Dining") == (D(0), D(35))
assert p40.pay() == 165 and p40.unassigned() == 1060
p40.rollover()
p40.row("Ex9 Dec1 after 40")
assert p40.avail("Dining") == 0 and p40.pay() == 165 and p40.owed - p40.pay() == 35 and p40.unassigned() == 1060

p75 = copy.deepcopy(m); p75.assign("Dining", 75)
assert p75.avail("Dining") == 0 and p75.pay() == 200 and p75.unassigned() == 1025

none = copy.deepcopy(m); none.rollover()
none.row("Ex9 Dec1 nothing corrected")
assert none.avail("Dining") == -30 and none.pay() == 155 and none.owed - none.pay() == 45 and none.unassigned() == 1100

# Timing: the same 40 assigned BEFORE the purchases is just funds
pre = copy.deepcopy(base); pre.assign("Dining", 40)
pre.spend_card("Dining", 200); pre.spend_cash("Dining", 30)
pre.row("Ex9 40 assigned Nov1 first")
assert pre.avail("Dining") == -35 and pre.split("Dining") == (D(30), D(5))
assert pre.pay() == 195 and pre.unassigned() == 1060

# Purchase order does not matter
def order_test(first):
    t = Budget(); t.cash = D(5000); t.assign("Dining", 100)
    ops = {"cash": lambda: t.spend_cash("Dining", 60), "card": lambda: t.spend_card("Dining", 70)}
    for k in first: ops[k]()
    return t.split("Dining"), t.pay(), t.unassigned()
assert order_test(["cash", "card"]) == order_test(["card", "cash"])
print("order test:", order_test(["cash", "card"]))

# Ex 10, 11
s = copy.deepcopy(base)
s.spend_cash("Groceries", -150); s.cash += 50
assert s.cash == 2935 and s.avail("Groceries") == 130 and s.unassigned() == 1150
mv = copy.deepcopy(base); mv.move("Rent", "Groceries", 20)
assert mv.avail("Rent") == 1480 and mv.avail("Groceries") == 0 and mv.unassigned() == 1100
assert mv.split("Groceries") == (D(0), D(0))
print("ALL CHECKS PASSED")
