//! `calc()` / `min()` / `max()` / `clamp()` evaluation into symbolic
//! [`CalcExpr`] trees (CSS Values 4). Mixed px/percent expressions stay
//! unresolved until layout supplies the percentage base.

use crate::values::{CalcExpr, Len};
use lightningcss::values::length::{LengthPercentage, LengthValue};
use lightningcss::values::percentage::DimensionPercentage as DP;

use crate::computed::LengthContext;

fn lp_to_expr(lp: &LengthPercentage, ctx: &LengthContext) -> CalcExpr {
    match lp {
        DP::Dimension(lv) => CalcExpr::Px(length_px(lv, ctx)),
        DP::Percentage(p) => CalcExpr::Percent(p.0),
        DP::Calc(c) => calc_to_expr(c, ctx),
    }
}

fn length_px(lv: &LengthValue, ctx: &LengthContext) -> f32 {
    crate::computed::length_to_px(lv, ctx).unwrap_or(0.0)
}

/// Convert a lightningcss calc node into a symbolic expression.
fn calc_to_expr(
    c: &lightningcss::values::calc::Calc<LengthPercentage>,
    ctx: &LengthContext,
) -> CalcExpr {
    use lightningcss::values::calc::{Calc, MathFunction};
    match c {
        Calc::Value(v) => lp_to_expr(v, ctx),
        Calc::Number(n) => CalcExpr::Px(*n),
        Calc::Sum(a, b) => {
            let (a, b) = (calc_to_expr(a, ctx), calc_to_expr(b, ctx));
            // Fold constant sums to keep trees small.
            if let (CalcExpr::Px(x), CalcExpr::Px(y)) = (&a, &b) {
                return CalcExpr::Px(x + y);
            }
            CalcExpr::Sum(Box::new(a), Box::new(b))
        }
        Calc::Product(n, v) => {
            let inner = calc_to_expr(v, ctx);
            if let CalcExpr::Px(x) = inner {
                CalcExpr::Px(*n * x)
            } else {
                CalcExpr::Product(*n, Box::new(inner))
            }
        }
        Calc::Function(f) => match f.as_ref() {
            MathFunction::Calc(inner) => calc_to_expr(inner, ctx),
            MathFunction::Min(args) => {
                let exprs: Vec<CalcExpr> = args.iter().map(|a| calc_to_expr(a, ctx)).collect();
                if exprs.iter().all(|e| matches!(e, CalcExpr::Px(_))) {
                    let m = exprs
                        .iter()
                        .filter_map(|e| match e {
                            CalcExpr::Px(v) => Some(*v),
                            _ => None,
                        })
                        .fold(f32::INFINITY, f32::min);
                    CalcExpr::Px(m)
                } else {
                    CalcExpr::Min(exprs)
                }
            }
            MathFunction::Max(args) => {
                let exprs: Vec<CalcExpr> = args.iter().map(|a| calc_to_expr(a, ctx)).collect();
                if exprs.iter().all(|e| matches!(e, CalcExpr::Px(_))) {
                    let m = exprs
                        .iter()
                        .filter_map(|e| match e {
                            CalcExpr::Px(v) => Some(*v),
                            _ => None,
                        })
                        .fold(f32::NEG_INFINITY, f32::max);
                    CalcExpr::Px(m)
                } else {
                    CalcExpr::Max(exprs)
                }
            }
            MathFunction::Clamp(min, val, max) => CalcExpr::Clamp(
                Box::new(calc_to_expr(min, ctx)),
                Box::new(calc_to_expr(val, ctx)),
                Box::new(calc_to_expr(max, ctx)),
            ),
            // round()/rem()/mod()/abs()/sign()/hypot(): fold to the first
            // argument's px part (rare in real sheets; documented).
            other => {
                let _ = other;
                CalcExpr::Px(0.0)
            }
        },
    }
}

/// Evaluate a `<length-percentage>` that may carry calc() into a [`Len`].
/// Pure numbers/percent keep the existing fast paths; math expressions
/// become `Len::Calc` and resolve at used-value time.
pub(crate) fn length_percentage_calc(lp: &LengthPercentage, ctx: &LengthContext) -> Option<Len> {
    match lp {
        DP::Dimension(lv) => crate::computed::length_to_px(lv, ctx).map(Len::Px),
        DP::Percentage(p) => Some(Len::Percent(p.0)),
        DP::Calc(c) => {
            let expr = calc_to_expr(c, ctx);
            // Simplify fully-px expressions eagerly.
            match &expr {
                CalcExpr::Px(v) => Some(Len::Px(*v)),
                _ => Some(Len::Calc(expr)),
            }
        }
    }
}
