#!/usr/bin/env python3
"""hypothesis_test — Engram skill (no network). Classic significance tests.

Runs a one-sample t-test, a Welch (unequal-variance) two-sample t-test, or a
chi-square test of independence on a contingency table. Two-tailed t p-values
use the regularized incomplete beta function; chi-square p-values use the
regularized incomplete gamma function (pure-Python Numerical-Recipes routines).

Request (stdin):
  {"test": "one_sample_t", "sample": [..], "popmean": 0}
  {"test": "two_sample_t", "sample1": [..], "sample2": [..]}
  {"test": "chi_square", "observed": [[..],[..]]}
Output (stdout): {test, statistic, df, p_value, "significant_at_0.05"}
"""
import json, sys, math, statistics


# --- regularized incomplete beta (for Student-t p-values) -------------------
def _betacf(a, b, x):
    MAXIT, EPS, FPMIN = 200, 3.0e-12, 1.0e-300
    qab, qap, qam = a + b, a + 1.0, a - 1.0
    c = 1.0
    d = 1.0 - qab * x / qap
    if abs(d) < FPMIN:
        d = FPMIN
    d = 1.0 / d
    h = d
    for m in range(1, MAXIT + 1):
        m2 = 2 * m
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1.0 + aa * d
        if abs(d) < FPMIN:
            d = FPMIN
        c = 1.0 + aa / c
        if abs(c) < FPMIN:
            c = FPMIN
        d = 1.0 / d
        h *= d * c
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1.0 + aa * d
        if abs(d) < FPMIN:
            d = FPMIN
        c = 1.0 + aa / c
        if abs(c) < FPMIN:
            c = FPMIN
        d = 1.0 / d
        de = d * c
        h *= de
        if abs(de - 1.0) < EPS:
            break
    return h


def _betai(a, b, x):
    if x <= 0.0:
        return 0.0
    if x >= 1.0:
        return 1.0
    bt = math.exp(math.lgamma(a + b) - math.lgamma(a) - math.lgamma(b)
                  + a * math.log(x) + b * math.log(1.0 - x))
    if x < (a + 1.0) / (a + b + 2.0):
        return bt * _betacf(a, b, x) / a
    return 1.0 - bt * _betacf(b, a, 1.0 - x) / b


def _t_two_tailed_p(t, df):
    # P(|T| > |t|) for T ~ Student-t(df).
    if df <= 0:
        return None
    return _betai(0.5 * df, 0.5, df / (df + t * t))


# --- regularized incomplete gamma (for chi-square p-values) -----------------
def _gser(a, x):
    ITMAX, EPS = 500, 3.0e-12
    gln = math.lgamma(a)
    if x <= 0.0:
        return 0.0
    ap = a
    summ = 1.0 / a
    dl = summ
    for _ in range(ITMAX):
        ap += 1.0
        dl *= x / ap
        summ += dl
        if abs(dl) < abs(summ) * EPS:
            break
    return summ * math.exp(-x + a * math.log(x) - gln)


def _gcf(a, x):
    ITMAX, EPS, FPMIN = 500, 3.0e-12, 1.0e-300
    gln = math.lgamma(a)
    b = x + 1.0 - a
    c = 1.0 / FPMIN
    d = 1.0 / b
    h = d
    for i in range(1, ITMAX + 1):
        an = -i * (i - a)
        b += 2.0
        d = an * d + b
        if abs(d) < FPMIN:
            d = FPMIN
        c = b + an / c
        if abs(c) < FPMIN:
            c = FPMIN
        d = 1.0 / d
        de = d * c
        h *= de
        if abs(de - 1.0) < EPS:
            break
    return math.exp(-x + a * math.log(x) - gln) * h


def _gammq(a, x):
    # Upper regularized incomplete gamma Q(a,x) = 1 - P(a,x).
    if x < 0.0 or a <= 0.0:
        raise ValueError("bad args in gammq")
    if x < a + 1.0:
        return 1.0 - _gser(a, x)
    return _gcf(a, x)


def _numlist(v):
    return isinstance(v, list) and len(v) > 0 and all(
        isinstance(t, (int, float)) and not isinstance(t, bool) for t in v)


def _one_sample_t(q):
    sample = q.get("sample")
    popmean = q.get("popmean")
    if not _numlist(sample):
        return {"error": "one_sample_t needs 'sample': a list of numbers",
                "example": {"test": "one_sample_t", "sample": [1, 2, 3, 4, 5], "popmean": 2}}
    if not isinstance(popmean, (int, float)) or isinstance(popmean, bool):
        return {"error": "one_sample_t needs numeric 'popmean'",
                "example": {"test": "one_sample_t", "sample": [1, 2, 3, 4, 5], "popmean": 2}}
    xs = [float(v) for v in sample]
    n = len(xs)
    if n < 2:
        return {"error": "one_sample_t needs at least 2 observations"}
    mean = statistics.fmean(xs)
    sd = statistics.stdev(xs)
    if sd == 0:
        return {"error": "sample has zero variance; t is undefined"}
    df = n - 1
    t = (mean - float(popmean)) / (sd / math.sqrt(n))
    p = _t_two_tailed_p(t, df)
    return {"test": "one_sample_t", "sample_mean": round(mean, 6),
            "popmean": float(popmean), "statistic": round(t, 6),
            "df": df, "p_value": round(p, 6),
            "significant_at_0.05": bool(p < 0.05)}


def _two_sample_t(q):
    s1 = q.get("sample1")
    s2 = q.get("sample2")
    if not _numlist(s1) or not _numlist(s2):
        return {"error": "two_sample_t needs 'sample1' and 'sample2': lists of numbers",
                "example": {"test": "two_sample_t", "sample1": [1, 2, 3, 4], "sample2": [3, 4, 5, 6]}}
    a = [float(v) for v in s1]
    b = [float(v) for v in s2]
    n1, n2 = len(a), len(b)
    if n1 < 2 or n2 < 2:
        return {"error": "two_sample_t needs at least 2 observations per sample"}
    m1, m2 = statistics.fmean(a), statistics.fmean(b)
    v1, v2 = statistics.variance(a), statistics.variance(b)
    if v1 == 0 and v2 == 0:
        return {"error": "both samples have zero variance; t is undefined"}
    se2 = v1 / n1 + v2 / n2
    t = (m1 - m2) / math.sqrt(se2)
    # Welch-Satterthwaite df.
    df = se2 ** 2 / ((v1 / n1) ** 2 / (n1 - 1) + (v2 / n2) ** 2 / (n2 - 1))
    p = _t_two_tailed_p(t, df)
    return {"test": "two_sample_t (Welch)", "mean1": round(m1, 6),
            "mean2": round(m2, 6), "statistic": round(t, 6),
            "df": round(df, 6), "p_value": round(p, 6),
            "significant_at_0.05": bool(p < 0.05)}


def _chi_square(q):
    obs = q.get("observed")
    ex = {"test": "chi_square", "observed": [[10, 20], [30, 40]]}
    if (not isinstance(obs, list) or len(obs) < 2 or
            not all(isinstance(r, list) and len(r) >= 2 for r in obs)):
        return {"error": "chi_square needs 'observed': a 2D contingency table (>=2 rows, >=2 cols)", "example": ex}
    ncol = len(obs[0])
    if not all(len(r) == ncol for r in obs):
        return {"error": "all rows of 'observed' must have the same length", "example": ex}
    for r in obs:
        for v in r:
            if not isinstance(v, (int, float)) or isinstance(v, bool) or v < 0:
                return {"error": "'observed' cells must be non-negative numbers", "example": ex}
    rows = [[float(v) for v in r] for r in obs]
    nrow = len(rows)
    row_tot = [sum(r) for r in rows]
    col_tot = [sum(rows[i][j] for i in range(nrow)) for j in range(ncol)]
    grand = sum(row_tot)
    if grand == 0:
        return {"error": "contingency table is all zeros"}
    chi2 = 0.0
    for i in range(nrow):
        for j in range(ncol):
            e = row_tot[i] * col_tot[j] / grand
            if e == 0:
                return {"error": "a marginal total is zero; expected count is 0 (chi-square undefined)"}
            chi2 += (rows[i][j] - e) ** 2 / e
    df = (nrow - 1) * (ncol - 1)
    p = _gammq(0.5 * df, 0.5 * chi2)
    return {"test": "chi_square", "statistic": round(chi2, 6), "df": df,
            "p_value": round(p, 6), "significant_at_0.05": bool(p < 0.05)}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"test": "one_sample_t", "sample": [1, 2, 3, 4, 5], "popmean": 2}})); return 0

    test = q.get("test")
    handlers = {"one_sample_t": _one_sample_t, "two_sample_t": _two_sample_t, "chi_square": _chi_square}
    if test not in handlers:
        print(json.dumps({
            "error": "'test' must be one of: one_sample_t, two_sample_t, chi_square",
            "example": {"test": "one_sample_t", "sample": [1, 2, 3, 4, 5], "popmean": 2},
        })); return 0

    try:
        result = handlers[test](q)
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "hypothesis_test failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
