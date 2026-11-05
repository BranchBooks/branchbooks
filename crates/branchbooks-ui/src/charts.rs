//! Charts as plain SVG: a line for one value over time,
//! paired columns for two, and a bar list for magnitudes. Every chart has a table view, so no value
//! is only in a picture. Amounts stay exact `Decimal`s; floats are used only to place pixels.

use dioxus::prelude::*;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::text::fmt;

const WIDTH: f64 = 640.0;
const HEIGHT: f64 = 220.0;
const PAD: (f64, f64, f64, f64) = (14.0, 16.0, 26.0, 48.0); // top, right, bottom, left

fn px(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
}

/// 1234 -> "1.2K", 1234567 -> "1.2M"; small numbers stay as they are.
pub fn compact(n: f64) -> String {
    let a = n.abs();
    if a >= 1e6 {
        format!("{:.*}M", usize::from(a < 1e7), n / 1e6)
    } else if a >= 1e3 {
        format!("{:.*}K", usize::from(a < 1e4), n / 1e3)
    } else {
        format!("{}", n.round())
    }
}

/// Round axis ticks covering [lo, hi], always including zero.
pub fn ticks(lo: f64, hi: f64) -> Vec<f64> {
    let (min, max) = (lo.min(0.0), hi.max(0.0));
    if min == max {
        return vec![0.0];
    }
    let raw = (max - min) / 4.0;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * mag);
    let mut out = Vec::new();
    let mut v = (min / step).floor() * step;
    while v <= max + step * 1e-9 {
        out.push((v / step).round() * step);
        v += step;
    }
    if out.last().is_some_and(|l| *l < max) {
        out.push(out[out.len() - 1] + step);
    }
    out
}

/// The y scale over `ticks`: value to pixel.
fn scale(t: &[f64]) -> impl Fn(f64) -> f64 + '_ {
    let (lo, hi) = (t[0], t[t.len() - 1]);
    move |v| {
        let span = if hi == lo { 1.0 } else { hi - lo };
        PAD.0 + (HEIGHT - PAD.0 - PAD.2) * (1.0 - (v - lo) / span)
    }
}

fn axis(t: &[f64]) -> Element {
    let y = scale(t);
    rsx! {
        g { class: "axis",
            for v in t.iter().copied() {
                line { class: if v == 0.0 { "baseline" } else { "grid" }, x1: PAD.3, x2: WIDTH - PAD.1, y1: y(v), y2: y(v) }
                text { x: PAD.3 - 6.0, y: y(v) + 4.0, "text-anchor": "end", {compact(v)} }
            }
        }
    }
}

/// A series to draw: a name, a CSS colour (a `var()` from style.css) and one value per label.
#[derive(Clone, PartialEq)]
pub struct Series {
    pub name: &'static str,
    pub color: &'static str,
    pub values: Vec<Decimal>,
}

/// A chart card: title, legend, then the chart or, after "Table", its rows (first column text,
/// the rest amounts).
#[component]
pub fn ChartCard(
    title: String,
    subtitle: String,
    #[props(default)] legend: Vec<Series>,
    head: Vec<&'static str>,
    rows: Vec<(String, Vec<Decimal>)>,
    children: Element,
) -> Element {
    let mut as_table = use_signal(|| false);
    rsx! {
        section { class: "chart-card", "aria-label": "{title}",
            div { class: "section-head",
                h2 { "{title}" span { class: "muted small-inline", " {subtitle}" } }
                button { r#type: "button", class: "link", "aria-pressed": as_table(),
                    onclick: move |_| as_table.toggle(),
                    if as_table() { "Chart" } else { "Table" }
                }
            }
            if !legend.is_empty() {
                ul { class: "legend",
                    for s in legend {
                        li { span { class: "key rect", style: "background: {s.color}", "aria-hidden": "true" } "{s.name}" }
                    }
                }
            }
            if as_table() {
                table { class: "chart-table",
                    thead { tr { for (i, h) in head.iter().enumerate() { th { class: if i > 0 { "num" } else { "" }, "{h}" } } } }
                    tbody {
                        for (label, values) in rows {
                            tr { td { "{label}" } for v in values { td { class: "num", {fmt(v)} } } }
                        }
                    }
                }
            } else {
                {children}
            }
        }
    }
}

/// One value over time, with the last value labelled.
#[component]
pub fn LineChart(labels: Vec<String>, series: Series) -> Element {
    let vals: Vec<f64> = series.values.iter().copied().map(px).collect();
    if vals.is_empty() {
        return rsx! { p { class: "muted", "Nothing to show yet." } };
    }
    let lo = vals.iter().copied().fold(f64::MAX, f64::min);
    let hi = vals.iter().copied().fold(f64::MIN, f64::max);
    let t = ticks(lo, hi);
    let y = scale(&t);
    let n = vals.len();
    let x = |i: usize| {
        PAD.3
            + (WIDTH - PAD.3 - PAD.1)
                * if n > 1 {
                    i as f64 / (n - 1) as f64
                } else {
                    0.5
                }
    };
    let d: String = vals
        .iter()
        .enumerate()
        .map(|(i, v)| format!("{}{:.1},{:.1}", if i == 0 { "M" } else { "L" }, x(i), y(*v)))
        .collect();
    let last = n - 1;
    rsx! {
        div { class: "chart",
            svg { view_box: "0 0 {WIDTH} {HEIGHT}", role: "img", "aria-label": "{series.name}",
                {axis(&t)}
                path { d: "{d}", class: "series-line", style: "stroke: {series.color}" }
                for (i, l) in labels.iter().enumerate() {
                    text { class: "axis-label", x: x(i), y: HEIGHT - 8.0, "text-anchor": "middle", "{l}" }
                }
                circle { cx: x(last), cy: y(vals[last]), r: 4.5, class: "end-dot", style: "fill: {series.color}" }
                text { class: "end-label", x: x(last), y: y(vals[last]) - 10.0, "text-anchor": "end", {fmt(series.values[last])} }
            }
        }
    }
}

/// Paired columns per label, one per series.
#[component]
pub fn ColumnChart(labels: Vec<String>, series: Vec<Series>) -> Element {
    let all: Vec<f64> = series
        .iter()
        .flat_map(|s| s.values.iter().copied().map(px))
        .collect();
    if all.is_empty() {
        return rsx! { p { class: "muted", "Nothing to show yet." } };
    }
    let t = ticks(
        all.iter().copied().fold(f64::MAX, f64::min),
        all.iter().copied().fold(f64::MIN, f64::max),
    );
    let y = scale(&t);
    let band = (WIDTH - PAD.3 - PAD.1) / labels.len().max(1) as f64;
    let bar = (band * 0.8 / series.len().max(1) as f64).min(28.0);
    rsx! {
        div { class: "chart",
            svg { view_box: "0 0 {WIDTH} {HEIGHT}", role: "img", "aria-label": "columns",
                {axis(&t)}
                for (i, l) in labels.iter().enumerate() {
                    g { class: "col",
                        for (k, s) in series.iter().enumerate() {
                            {
                                let v = px(s.values.get(i).copied().unwrap_or_default());
                                let left = PAD.3 + band * i as f64 + (band - bar * series.len() as f64) / 2.0 + bar * k as f64;
                                let (top, bottom) = (y(v.max(0.0)), y(v.min(0.0)));
                                rsx! {
                                    rect { x: left, y: top, width: bar - 2.0, height: (bottom - top).max(0.5), rx: 2,
                                        style: "fill: {s.color}",
                                        title { "{s.name} {l}: {fmt(s.values.get(i).copied().unwrap_or_default())}" }
                                    }
                                }
                            }
                        }
                        text { class: "axis-label", x: PAD.3 + band * i as f64 + band / 2.0, y: HEIGHT - 8.0, "text-anchor": "middle", "{l}" }
                    }
                }
            }
        }
    }
}

/// Magnitudes as horizontal bars, largest first.
#[component]
pub fn BarList(rows: Vec<(String, Decimal)>, color: &'static str) -> Element {
    let max = rows.iter().map(|(_, v)| px(*v)).fold(0.0, f64::max);
    let max = if max > 0.0 { max } else { 1.0 };
    rsx! {
        ul { class: "bar-list",
            for (label, v) in rows {
                li { title: "{label}: {fmt(v)}",
                    span { class: "bar-label", "{label}" }
                    span { class: "bar-track",
                        span { class: "bar-fill", style: "width: {(px(v) / max * 100.0).max(0.5):.1}%; background: {color}" }
                    }
                    span { class: "bar-value", {fmt(v)} }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_cover_the_range_and_zero_and_numbers_shorten() {
        assert_eq!(ticks(0.0, 950.0), vec![0.0, 250.0, 500.0, 750.0, 1000.0]);
        assert_eq!(
            ticks(-120.0, 80.0),
            vec![-150.0, -100.0, -50.0, 0.0, 50.0, 100.0]
        );
        assert_eq!(ticks(5.0, 5.0), vec![0.0, 2.0, 4.0, 6.0]);
        assert_eq!(compact(1234.0), "1.2K");
        assert_eq!(compact(56_000.0), "56K");
        assert_eq!(compact(2_500_000.0), "2.5M");
        assert_eq!(compact(-42.4), "-42");
    }
}
