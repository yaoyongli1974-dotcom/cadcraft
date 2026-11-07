//! The offline rule engine: deterministic Chinese/English templates, no network.
//!
//! Scope (v1): basic drawing verbs - line, rectangle, circle, arc and text - plus
//! moving, rotating, mirroring and scaling the selection (or the whole drawing
//! where the context samples every object), changing the radius of a circle,
//! layer creation/current layer/moving the selection to a layer, and offsetting
//! a selected circle. Anything else returns a "no rule" error mentioning an
//! example phrasing, so the UI can suggest switching to an OpenAI-compatible
//! provider.
//!
//! Every rule is a pure function of `(request, context)`: it extracts keywords
//! and numbers from the request and reads entity handles from the context block
//! produced by [`crate::assistant::context`]. It never touches a `Session` and
//! never guesses an unsupported command.

use serde_json::{Map, Value, json};

use crate::assistant::Draft;

/// Entry point used by `provider::generate` when `AiMode::Offline`.
pub fn offline_draft(user: &str, context: &str) -> Result<Draft, String> {
    let request = normalize(user);
    if request.trim().is_empty() {
        return Err(String::from("输入为空 / empty request"));
    }
    let brief = ContextBrief::from(context);
    for rule in RULES {
        if let Some(outcome) = rule(&request, &brief) {
            return outcome;
        }
    }
    Err(no_rule(user.trim()))
}

/// The rules, in the order they are tried. A rule returns `None` when the
/// request is not about it, `Some(Err(..))` when it is but cannot be honoured
/// (the message carries an example), and `Some(Ok(draft))` on success.
type Rule = fn(&str, &ContextBrief) -> Option<Result<Draft, String>>;

const RULES: &[Rule] = &[
    rule_radius_change,
    rule_layer_new,
    rule_layer_move,
    rule_layer_switch,
    rule_arc,
    rule_line,
    rule_rectangle,
    rule_circle,
    rule_text,
    rule_offset,
    rule_rotate,
    rule_mirror,
    rule_scale,
    rule_move,
];

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// One entity from the drawing-context sample: `1A Circle [0] {"center":…}`.
#[derive(Debug, Clone)]
struct Sample {
    handle: String,
    kind: String,
    geometry: Option<Value>,
}

impl Sample {
    /// `(center x, center y, radius)` when this sample is a circle with readable geometry.
    fn circle(&self) -> Option<(f64, f64, f64)> {
        if !self.kind.eq_ignore_ascii_case("Circle") {
            return None;
        }
        let g = self.geometry.as_ref()?;
        let center = g.get("center")?;
        let x = center.get("x")?.as_f64()?;
        let y = center.get("y")?.as_f64()?;
        let r = g.get("radius")?.as_f64()?;
        (x.is_finite() && y.is_finite() && r.is_finite() && r > 0.0).then_some((x, y, r))
    }
}

/// The pieces of the drawing context rules may consult.
#[derive(Debug, Default, Clone)]
struct ContextBrief {
    entities: Vec<Sample>,
    selection: Vec<String>,
    layers: Vec<String>,
    entity_count: Option<usize>,
    extents: Option<(f64, f64, f64, f64)>,
}

impl ContextBrief {
    fn from(context: &str) -> Self {
        let mut b = Self::default();
        for raw in context.lines() {
            let line = raw.trim();
            if let Some(rest) = line.strip_prefix("entityCount:") {
                b.entity_count = rest.trim().parse().ok();
            } else if let Some(rest) = line.strip_prefix("extents:") {
                b.extents = parse_extents(rest.trim());
            } else if let Some(rest) = line.strip_prefix("layers:") {
                b.layers = rest.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).collect();
            } else if let Some(rest) = line.strip_prefix("selection (") {
                if let Some((_, handles)) = rest.split_once("):") {
                    b.selection = handle_list(handles);
                }
            } else if let Some(rest) = line.strip_prefix("selection:") {
                b.selection = handle_list(rest);
            } else if let Some(sample) = parse_entity_line(line) {
                b.entities.push(sample);
            }
        }
        b
    }

    fn center(&self) -> Option<(f64, f64)> {
        let (x0, y0, x1, y1) = self.extents?;
        Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0))
    }

    fn circles(&self) -> Vec<&Sample> {
        self.entities.iter().filter(|e| e.circle().is_some()).collect()
    }

    /// Circles in the current selection, in context order.
    fn selected_circles(&self) -> Vec<&Sample> {
        self.entities.iter().filter(|e| self.selection.iter().any(|h| h.eq_ignore_ascii_case(&e.handle))).filter(|e| e.circle().is_some()).collect()
    }
}

fn handle_list(s: &str) -> Vec<String> {
    if s.contains("empty") {
        return Vec::new();
    }
    s.split_whitespace().map(|t| t.trim_matches(|c: char| !c.is_ascii_hexdigit())).filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

/// `1A Circle [0] {"center":…}`. The geometry may be truncated in the context
/// text; then the handle and kind are still kept.
fn parse_entity_line(line: &str) -> Option<Sample> {
    let mut parts = line.splitn(3, ' ');
    let handle = parts.next()?.trim();
    let kind = parts.next()?.trim();
    let rest = parts.next()?.trim_start();
    if handle.is_empty() || !handle.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    if !kind.chars().next().is_some_and(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    if !rest.starts_with('[') {
        return None;
    }
    let end = rest.find(']')?;
    let geometry = serde_json::from_str(rest[end + 1..].trim()).ok();
    Some(Sample { handle: handle.to_owned(), kind: kind.to_owned(), geometry })
}

/// `[[x0, y0], [x1, y1]]` from a context line.
fn parse_extents(s: &str) -> Option<(f64, f64, f64, f64)> {
    let v: Value = serde_json::from_str(s).ok()?;
    let a = v.as_array()?;
    let first = a.first()?.as_array()?;
    let second = a.get(1)?.as_array()?;
    Some((first.first()?.as_f64()?, first.get(1)?.as_f64()?, second.first()?.as_f64()?, second.get(1)?.as_f64()?))
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

/// Map full-width punctuation to ASCII so one parser handles both scripts.
/// ASCII case is preserved: layer names and text content keep their spelling.
fn normalize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        out.push(match c {
            '，' | '、' => ',',
            '（' => '(',
            '）' => ')',
            '：' => ':',
            '；' => ';',
            '。' => '.',
            '　' => ' ',
            '−' | '–' | '—' => '-',
            '×' => 'x',
            '“' | '”' | '「' | '」' => '"',
            '‘' | '’' => '\'',
            _ => c,
        });
    }
    out
}

fn is_zh(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

fn say(req: &str, cn: String, en: String) -> String {
    if is_zh(req) { cn } else { en }
}

/// Case-insensitive byte position of `needle`; ASCII case folding preserves
/// byte offsets, so the result slices `hay` safely.
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    hay.to_ascii_lowercase().find(&needle.to_ascii_lowercase())
}

fn has_word(s: &str, word: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t.eq_ignore_ascii_case(word))
}

fn find_word_pos(s: &str, word: &str) -> Option<usize> {
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if !c.is_ascii_alphanumeric() {
            if i > start && s[start..i].eq_ignore_ascii_case(word) {
                return Some(start);
            }
            start = i + c.len_utf8();
        }
    }
    (start < s.len() && s[start..].eq_ignore_ascii_case(word)).then_some(start)
}

/// First number at or after the start of `s`.
fn first_number(s: &str) -> Option<f64> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if let Some((v, _)) = number_at(&chars, i) {
            return Some(v);
        }
        i += 1;
    }
    None
}

/// A number starting exactly at `i`, with the index just past it.
fn number_at(chars: &[char], i: usize) -> Option<(f64, usize)> {
    let c = *chars.get(i)?;
    let starts = c.is_ascii_digit() || ((c == '-' || c == '+' || c == '.') && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit()));
    if !starts {
        return None;
    }
    let mut j = i;
    if c == '-' || c == '+' {
        j += 1;
    }
    let mut dot = false;
    while let Some(d) = chars.get(j) {
        if d.is_ascii_digit() {
            j += 1;
        } else if *d == '.' && !dot {
            dot = true;
            j += 1;
        } else {
            break;
        }
    }
    let text: String = chars.get(i..j)?.iter().collect();
    Some((text.parse::<f64>().ok()?, j))
}

/// First number after the first occurrence of any key (keys are tried in order).
fn number_after(s: &str, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(pos) = find_ci(s, key)
            && let Some(tail) = s.get(pos + key.len()..)
            && let Some(n) = first_number(tail)
        {
            return Some(n);
        }
    }
    None
}

/// All `x,y` pairs in the request, including `(x,y)`.
fn find_points(s: &str) -> Vec<(f64, f64)> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if let Some((x, next)) = number_at(&chars, i) {
            let mut j = next;
            while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                j += 1;
            }
            if chars.get(j) == Some(&',') {
                j += 1;
                while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                    j += 1;
                }
                if let Some((y, end)) = number_at(&chars, j) {
                    out.push((x, y));
                    i = end;
                    continue;
                }
            }
            i = next;
        } else {
            i += 1;
        }
    }
    out
}

/// `120x40` (also `120 x 40`) in the request.
fn size_pair(s: &str) -> Option<(f64, f64)> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if let Some((a, next)) = number_at(&chars, i) {
            let mut j = next;
            while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                j += 1;
            }
            if chars.get(j).is_some_and(|c| *c == 'x' || *c == 'X') {
                j += 1;
                while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                    j += 1;
                }
                if let Some((b, _)) = number_at(&chars, j) {
                    return Some((a, b));
                }
            }
            i = next;
        } else {
            i += 1;
        }
    }
    None
}

/// First number between a start key and the following end key (`从 30 到 120`).
fn angle_pair(s: &str) -> Option<(f64, f64)> {
    for (start_key, end_key) in [("从", "到"), ("from", "to"), ("start", "end")] {
        if let Some(pos) = find_ci(s, start_key) {
            let tail = s.get(pos + start_key.len()..)?;
            if let Some(epos) = find_ci(tail, end_key) {
                let start = first_number(&tail[..epos])?;
                let end = first_number(&tail[epos + end_key.len()..])?;
                return Some((start, end));
            }
        }
    }
    None
}

fn num(v: f64) -> String {
    if v == 0.0 {
        return String::from("0");
    }
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

fn point_text(x: f64, y: f64) -> String {
    format!("{},{}", num(x), num(y))
}

fn draft(reply: String, script: String) -> Draft {
    Draft { reply, script }
}

fn no_rule(request: &str) -> String {
    format!(
        "离线规则不认识这句话 / offline rules cannot parse: {request}。\
例 / try: 在 0,0 到 100,40 画线; 画宽 120 高 40 的矩形; 在 100,100 画半径 60 的圆; \
以 0,0 为圆心画半径 50 从 30 到 120 的圆弧; 在 10,20 写 你好; 把选中的向上移动 10; \
把选中的旋转 45 度; 左右镜像选中; 把选中的放大 2 倍; 新建红层 Walls; 切换到 Walls; \
把选中的移到 Walls 层。复杂表达请在设置中接入 OpenAI 兼容大模型 / \
connect an OpenAI-compatible provider in settings"
    )
}

/// `没有选中对象` in the request's language.
fn need_selection(req: &str) -> String {
    say(
        req,
        String::from("当前没有选中对象 / nothing is selected: 请先在图中选中对象，或说“选中所有实体”"),
        String::from("Nothing is selected: select objects in the drawing first"),
    )
}

fn is_all_scope(req: &str) -> bool {
    req.contains("所有") || req.contains("全部") || req.contains("整个图纸") || has_word(req, "all") || has_word(req, "everything")
}

fn is_selection_scope(req: &str) -> bool {
    req.contains("选中") || req.contains("选择") || has_word(req, "selection") || has_word(req, "selected")
}

fn out_of_range(what: &str, req: &str) -> String {
    say(req, format!("{what} 必须为正数 / {what} must be positive"), format!("{what} must be positive"))
}

// ---------------------------------------------------------------------------
// Rules: draw
// ---------------------------------------------------------------------------

/// `在 0,0 到 100,40 画线` / `draw a line from 0,0 to 100,40`.
fn rule_line(req: &str, _b: &ContextBrief) -> Option<Result<Draft, String>> {
    let zh = req.contains("画线") || req.contains("直线") || req.contains("线段") || req.contains("画一条线");
    if !zh && !has_word(req, "line") {
        return None;
    }
    let pts = find_points(req);
    let (Some((x0, y0)), Some((x1, y1))) = (pts.first(), pts.get(1)) else {
        return Some(Err(say(
            req,
            String::from("请给出两个点 / need two points: 例 在 0,0 到 100,40 画线"),
            String::from("Need two points: try `draw a line from 0,0 to 100,40`"),
        )));
    };
    let script = format!("line {}", json!({ "points": [[*x0, *y0], [*x1, *y1]] }));
    let reply = say(
        req,
        format!("画线: ({}) → ({})", point_text(*x0, *y0), point_text(*x1, *y1)),
        format!("Line from ({}) to ({})", point_text(*x0, *y0), point_text(*x1, *y1)),
    );
    Some(Ok(draft(reply, script)))
}

/// `画宽 120 高 40 的矩形` / `rect 120x40 at 10,20`.
fn rule_rectangle(req: &str, _b: &ContextBrief) -> Option<Result<Draft, String>> {
    if !(req.contains("矩形") || req.contains("长方形") || has_word(req, "rect") || has_word(req, "rectangle")) {
        return None;
    }
    let pair = size_pair(req);
    let width = pair.map(|(w, _)| w).or_else(|| number_after(req, &["宽", "width"]));
    let height = pair.map(|(_, h)| h).or_else(|| number_after(req, &["高", "height"]));
    let (Some(w), Some(h)) = (width, height) else {
        return Some(Err(say(
            req,
            String::from("请给出宽和高 / need width and height: 例 画宽 120 高 40 的矩形; rect 120x40 at 0,0"),
            String::from("Need width and height: try `rect 120x40 at 0,0`"),
        )));
    };
    if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
        return Some(Err(out_of_range("宽和高 / width and height", req)));
    }
    let (x, y) = find_points(req).first().copied().unwrap_or((0.0, 0.0));
    let script = format!("rectang {}", json!({ "p1": [x, y], "p2": [x + w, y + h] }));
    let reply = say(
        req,
        format!("画矩形: 左下角 ({}), 宽 {}, 高 {}", point_text(x, y), num(w), num(h)),
        format!("Rectangle at ({}), {} x {}", point_text(x, y), num(w), num(h)),
    );
    Some(Ok(draft(reply, script)))
}

/// `以 (100,100) 为圆心画半径 60 的圆` / `circle 30 at 10,10`.
fn rule_circle(req: &str, _b: &ContextBrief) -> Option<Result<Draft, String>> {
    let mentions_circle = (req.contains('圆') && !req.contains("圆弧")) || has_word(req, "circle");
    if !mentions_circle {
        return None;
    }
    // "把选中的圆向外偏移" and "把圆的半径改掉" are other rules.
    let creates = req.contains("画") || req.contains("创建") || req.contains("绘制") || has_word(req, "draw") || has_word(req, "circle");
    if !creates {
        return None;
    }
    let Some((x, y)) = find_points(req).first().copied() else {
        return Some(Err(say(
            req,
            String::from("请给出圆心 / need the centre: 例 在 100,100 画半径 60 的圆; circle 30 at 10,10"),
            String::from("Need the centre: try `circle 30 at 10,10`"),
        )));
    };
    let radius = number_after(req, &["半径", "radius"]).or_else(|| {
        let pos = find_word_pos(req, "circle")?;
        first_number(req.get(pos + "circle".len()..)?)
    });
    let Some(r) = radius else {
        return Some(Err(say(
            req,
            String::from("请给出半径 / need the radius: 例 在 100,100 画半径 60 的圆; circle 30 at 10,10"),
            String::from("Need the radius: try `circle 30 at 10,10`"),
        )));
    };
    if !(r > 0.0 && r.is_finite()) {
        return Some(Err(out_of_range("半径 / radius", req)));
    }
    let reply =
        say(req, format!("画圆: 圆心 ({}), 半径 {}", point_text(x, y), num(r)), format!("Circle at ({}), radius {}", point_text(x, y), num(r)));
    Some(Ok(draft(reply, format!("circle {} {}\n", point_text(x, y), num(r)))))
}

/// `以 (0,0) 为圆心画半径 50 从 30 到 120 的圆弧` / `arc center 0,0 radius 50 from 30 to 120`.
fn rule_arc(req: &str, _b: &ContextBrief) -> Option<Result<Draft, String>> {
    if !(req.contains("圆弧") || has_word(req, "arc")) {
        return None;
    }
    let Some((x, y)) = find_points(req).first().copied() else {
        return Some(Err(say(
            req,
            String::from("请给出圆心 / need the centre: 例 以 0,0 为圆心画半径 50 从 30 到 120 的圆弧"),
            String::from("Need the centre: try `arc center 0,0 radius 50 from 30 to 120`"),
        )));
    };
    let radius = number_after(req, &["半径", "radius"]);
    let angles = angle_pair(req);
    let (Some(r), Some((start, end))) = (radius, angles) else {
        return Some(Err(say(
            req,
            String::from("请给出半径和起止角度 / need radius and start/end angles: 例 以 0,0 为圆心画半径 50 从 30 到 120 的圆弧"),
            String::from("Need radius and angles: try `arc center 0,0 radius 50 from 30 to 120`"),
        )));
    };
    if !(r > 0.0 && r.is_finite()) {
        return Some(Err(out_of_range("半径 / radius", req)));
    }
    let script = format!("arc {}", json!({ "center": [x, y], "radius": r, "start": start, "end": end }));
    let reply = say(
        req,
        format!("画圆弧: 圆心 ({}), 半径 {}, 从 {}° 到 {}°", point_text(x, y), num(r), num(start), num(end)),
        format!("Arc at ({}), radius {}, {}° to {}°", point_text(x, y), num(r), num(start), num(end)),
    );
    Some(Ok(draft(reply, script)))
}

/// `在 (10,20) 写 你好` / `text "Hello" at 10,20`.
fn rule_text(req: &str, _b: &ContextBrief) -> Option<Result<Draft, String>> {
    let marker = has_word(req, "text") || has_word(req, "write") || req.contains("写") || req.contains("文字");
    if !marker {
        return None;
    }
    let Some((x, y)) = find_points(req).first().copied() else {
        return Some(Err(say(
            req,
            String::from("请给出文字位置 / need the insertion point: 例 在 10,20 写 你好"),
            String::from("Need the insertion point: try `text \"Hello\" at 10,20`"),
        )));
    };
    let Some(content) = text_content(req) else {
        return Some(Err(say(
            req,
            String::from("请给出要写的文字 / need the text: 例 在 10,20 写 你好"),
            String::from("Need the text: try `text \"Hello\" at 10,20`"),
        )));
    };
    let script = format!("text {}", json!({ "at": [x, y], "text": content.clone() }));
    let reply = say(req, format!("在 ({}) 写文字: {content}", point_text(x, y)), format!("Text at ({}): {content}", point_text(x, y)));
    Some(Ok(draft(reply, script)))
}

fn text_content(req: &str) -> Option<String> {
    if let Some(q) = quoted(req)
        && !q.is_empty()
    {
        return Some(q);
    }
    for marker in ["写文字", "写上", "写", "文字"] {
        if let Some(pos) = req.find(marker)
            && let Some(t) = req.get(pos + marker.len()..).and_then(clean_content)
        {
            return Some(t);
        }
    }
    for marker in ["text", "write"] {
        if let Some(pos) = find_word_pos(req, marker)
            && let Some(t) = req.get(pos + marker.len()..).and_then(clean_content)
        {
            return Some(t);
        }
    }
    None
}

/// Trim separators, quotes and a trailing `at x,y` / `在 x,y` clause.
fn clean_content(s: &str) -> Option<String> {
    let mut t = s.trim();
    t = t.trim_start_matches([':', '：', ',', '，', ' ', '\t']);
    t = t.trim_matches(['"', '\'', '“', '”', '「', '」']);
    for key in [" at ", " @ ", " 在 "] {
        if let Some(pos) = find_ci(t, key) {
            t = &t[..pos];
        }
    }
    let t = t.trim().trim_matches(['"', '\'', '“', '”', '「', '」']).trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn quoted(s: &str) -> Option<String> {
    let start = s.find(['"', '\''])?;
    let quote = s.get(start..start + 1)?;
    let rest = s.get(start + 1..)?;
    let end = rest.find(quote)?;
    Some(rest.get(..end)?.trim().to_owned())
}

// ---------------------------------------------------------------------------
// Rules: modify circles, layers, selection
// ---------------------------------------------------------------------------

/// `把最大的圆半径改成 25` / `set the radius of the selected circle to 15`.
fn rule_radius_change(req: &str, b: &ContextBrief) -> Option<Result<Draft, String>> {
    let about_circle = req.contains('圆') || has_word(req, "circle");
    let about_radius = req.contains("半径") || has_word(req, "radius");
    let changes = req.contains("改成")
        || req.contains("改为")
        || req.contains("设为")
        || req.contains("设置为")
        || req.contains("变成")
        || has_word(req, "set")
        || has_word(req, "change");
    if !(about_circle && about_radius && changes) {
        return None;
    }
    let Some(radius) = number_after(req, &["改成", "改为", "设为", "设置为", "变成", "radius", "to"]) else {
        return Some(Err(say(
            req,
            String::from("请给出新半径 / need the new radius: 例 把最大的圆半径改成 25"),
            String::from("Need the new radius: try `set the largest circle radius to 25`"),
        )));
    };
    if !(radius > 0.0 && radius.is_finite()) {
        return Some(Err(out_of_range("半径 / radius", req)));
    }
    let want_largest = req.contains("最大") || req.contains("外圆") || has_word(req, "largest") || has_word(req, "outer");
    let circles = b.circles();
    let target = if is_selection_scope(req) {
        b.selected_circles().first().copied()
    } else if want_largest {
        circles.iter().copied().max_by(|a, c| {
            let ra = a.circle().map(|t| t.2).unwrap_or(0.0);
            let rc = c.circle().map(|t| t.2).unwrap_or(0.0);
            ra.total_cmp(&rc)
        })
    } else if circles.len() == 1 {
        circles.first().copied()
    } else {
        None
    };
    let Some(sample) = target else {
        return Some(Err(say(
            req,
            String::from(
                "找不到要改的圆 / no circle to change: 请选中一个圆，说“把最大的圆半径改成 25”，\
或先画出圆（例 在 0,0 画半径 10 的圆）",
            ),
            String::from("No circle to change: select one, say `set the largest circle radius to 25`, or draw one first"),
        )));
    };
    let script = format!("properties.set {}", json!({ "handles": [sample.handle.clone()], "radius": radius }));
    let reply =
        say(req, format!("把圆 {} 的半径改为 {}", sample.handle, num(radius)), format!("Set circle {} radius to {}", sample.handle, num(radius)));
    Some(Ok(draft(reply, script)))
}

/// `新建红层 Walls` / `new layer Walls red`.
fn rule_layer_new(req: &str, _b: &ContextBrief) -> Option<Result<Draft, String>> {
    let zh = (req.contains("新建") || req.contains("创建") || req.contains("添加")) && req.contains('层');
    let en = (has_word(req, "new") || has_word(req, "create") || has_word(req, "add")) && has_word(req, "layer");
    if !(zh || en) {
        return None;
    }
    let Some(name) = layer_new_name(req) else {
        return Some(Err(say(
            req,
            String::from("请给出层名 / need a layer name: 例 新建红层 Walls; 新建绿色层 标注"),
            String::from("Need a layer name (层名): try `new layer Walls red`"),
        )));
    };
    let mut params = Map::new();
    params.insert(String::from("name"), json!(name.clone()));
    if let Some(color) = color_of(req) {
        params.insert(String::from("color"), json!(color));
    }
    let script = format!("layer.new {}", Value::Object(params));
    let reply = say(
        req,
        format!("新建图层 {name}{}", color_of(req).map(|c| format!("，颜色 {c}")).unwrap_or_default()),
        format!("New layer {name}{}", color_of(req).map(|c| format!(", colour {c}")).unwrap_or_default()),
    );
    Some(Ok(draft(reply, script)))
}

fn layer_new_name(s: &str) -> Option<String> {
    if let Some(pos) = find_ci(s, "图层")
        && let Some(n) = name_token(s.get(pos + "图层".len()..)?)
    {
        return Some(n);
    }
    if let Some(pos) = find_ci(s, "层") {
        if let Some(n) = name_token(s.get(pos + "层".len()..)?) {
            return Some(n);
        }
        if let Some(name) = s.get(..pos).and_then(last_name_token) {
            return Some(name);
        }
    }
    if let Some(pos) = find_word_pos(s, "layer") {
        let tail = s.get(pos + "layer".len()..)?;
        let mut tokens = tail.split_whitespace();
        if let Some(tok) = tokens.next() {
            let n = strip_layer_suffix(&clean_token(tok));
            if valid_layer_name(&n) {
                return Some(n);
            }
            if is_color_word(&n)
                && let Some(tok2) = tokens.next()
            {
                let n2 = strip_layer_suffix(&clean_token(tok2));
                if valid_layer_name(&n2) {
                    return Some(n2);
                }
            }
        }
        if let Some(name) = s.get(..pos).and_then(last_name_token) {
            return Some(name);
        }
    }
    None
}

fn name_token(s: &str) -> Option<String> {
    let n = strip_layer_suffix(&clean_token(s.split_whitespace().next()?));
    valid_layer_name(&n).then_some(n)
}

fn last_name_token(s: &str) -> Option<String> {
    let n = strip_layer_suffix(&clean_token(s.split_whitespace().last()?));
    valid_layer_name(&n).then_some(n)
}

fn valid_layer_name(n: &str) -> bool {
    !n.is_empty()
        && !n.starts_with("新建")
        && !n.starts_with("创建")
        && !n.starts_with("添加")
        && !is_color_word(n)
        && !matches!(
            n.to_ascii_lowercase().as_str(),
            "layer" | "图层" | "层" | "new" | "create" | "add" | "一个" | "新建" | "创建" | "添加" | "colour" | "color"
        )
}

fn strip_layer_suffix(name: &str) -> String {
    let mut n = name.trim().to_owned();
    for suffix in ["图层", "层"] {
        if let Some(rest) = n.strip_suffix(suffix) {
            n = rest.trim().to_owned();
            break;
        }
    }
    let lower = n.to_ascii_lowercase();
    if lower.ends_with(" layer") {
        n.truncate(n.len() - " layer".len());
        n = n.trim().to_owned();
    }
    n
}

fn clean_token(t: &str) -> String {
    t.trim_matches(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ',' | '.' | ';' | ':' | '，' | '。' | '"' | '\'' | '“' | '”' | '‘' | '’' | '(' | ')' | '（' | '）' | '[' | ']' | '《' | '》'
            )
    })
    .to_owned()
}

fn color_of(req: &str) -> Option<&'static str> {
    const COLORS: &[(&[&str], &str)] = &[
        (&["洋红", "品红", "magenta"], "magenta"),
        (&["灰", "gray", "grey"], "8"),
        (&["红", "red"], "red"),
        (&["绿", "green"], "green"),
        (&["蓝", "blue"], "blue"),
        (&["黄", "yellow"], "yellow"),
        (&["青", "cyan"], "cyan"),
        (&["白", "white"], "white"),
        (&["黑", "black"], "black"),
    ];
    for (words, name) in COLORS {
        if words.iter().any(|w| req.contains(w) || has_word(req, w)) {
            return Some(name);
        }
    }
    None
}

fn is_color_word(word: &str) -> bool {
    const WORDS: &[&str] = &[
        "红", "绿", "蓝", "黄", "青", "白", "黑", "灰", "洋红", "品红", "red", "green", "blue", "yellow", "cyan", "white", "black", "gray", "grey",
        "magenta",
    ];
    WORDS.iter().any(|w| word.eq_ignore_ascii_case(w))
}

/// `切换到 Walls` / `set current layer Walls`.
fn rule_layer_switch(req: &str, b: &ContextBrief) -> Option<Result<Draft, String>> {
    let zh = req.contains("切换") || req.contains("当前层") || req.contains("当前图层") || req.contains("设为当前");
    let en = has_word(req, "switch") || (has_word(req, "current") && has_word(req, "layer")) || has_word(req, "use");
    if !(zh || en) {
        return None;
    }
    let name = layer_after(req, &["切换到", "切换至", "切换", "设置为", "设为", "设成", "当前图层", "当前层", "to layer", "layer"]);
    let Some(name) = name else {
        return Some(Err(say(
            req,
            String::from("请给出层名 / need a layer name: 例 切换到 Walls"),
            String::from("Need a layer name (层名): try `set current layer Walls`"),
        )));
    };
    if let Err(e) = check_layer(b, &name) {
        return Some(Err(e));
    }
    let script = format!("layer.current {}", json!({ "name": &name }));
    let reply = say(req, format!("当前图层切换为 {name}"), format!("Current layer is now {name}"));
    Some(Ok(draft(reply, script)))
}

/// `把选中的移到 Walls 层` / `move selection to layer Walls`.
fn rule_layer_move(req: &str, b: &ContextBrief) -> Option<Result<Draft, String>> {
    let zh = (req.contains("移到") || req.contains("移至") || req.contains("移动到")) && req.contains('层');
    let en = has_word(req, "move") && has_word(req, "layer");
    if !(zh || en) {
        return None;
    }
    let Some(name) = layer_after(req, &["移动到", "移到", "移至", "to layer"]) else {
        return Some(Err(say(
            req,
            String::from("请给出目标层 / need the target layer: 例 把选中的移到 Walls 层"),
            String::from("Need a layer name (层名): try `move selection to layer Walls`"),
        )));
    };
    if b.selection.is_empty() {
        return Some(Err(need_selection(req)));
    }
    if let Err(e) = check_layer(b, &name) {
        return Some(Err(e));
    }
    let script = format!("laymch {}", json!({ "handles": &b.selection, "layer": &name }));
    let reply = say(
        req,
        format!("把 {} 个选中对象移到图层 {name}", b.selection.len()),
        format!("Moved {} selected object(s) to layer {name}", b.selection.len()),
    );
    Some(Ok(draft(reply, script)))
}

fn layer_after(s: &str, keys: &[&str]) -> Option<String> {
    for key in keys {
        let Some(pos) = find_ci(s, key) else { continue };
        let Some(tail) = s.get(pos + key.len()..) else { continue };
        let token = clean_token(tail.split_whitespace().next().unwrap_or_default());
        let n = strip_layer_suffix(&token);
        if valid_layer_name(&n) {
            return Some(n);
        }
    }
    None
}

fn check_layer(b: &ContextBrief, name: &str) -> Result<(), String> {
    if b.layers.is_empty() || b.layers.iter().any(|l| l.eq_ignore_ascii_case(name)) {
        return Ok(());
    }
    Err(format!(
        "图层 `{name}` 不存在 / no layer `{name}` (现有 / known: {})。\
先新建再使用 / create it first: 例 新建红层 {name}",
        b.layers.join(", ")
    ))
}

/// `把选中的向上移动 10` / `move selection up 10`.
fn rule_move(req: &str, b: &ContextBrief) -> Option<Result<Draft, String>> {
    if !(req.contains("移动") || req.contains("平移") || has_word(req, "move")) {
        return None;
    }
    let Some((dx, dy, word)) = direction(req) else {
        return Some(Err(say(
            req,
            String::from("请说明方向 / need a direction: 例 把选中的向上移动 10; 把所有实体向下移动 10"),
            String::from("Need a direction: try `move selection up 10` or `move all down 10`"),
        )));
    };
    let distance = number_after(req, &["移动", "move", "by", word]).or_else(|| first_number(req));
    let Some(distance) = distance else {
        return Some(Err(say(
            req,
            String::from("请给出距离 / need a distance: 例 把选中的向上移动 10"),
            String::from("Need a distance: try `move selection up 10`"),
        )));
    };
    if !(distance > 0.0 && distance.is_finite()) {
        return Some(Err(out_of_range("距离 / distance", req)));
    }
    let delta = [dx * distance, dy * distance];
    let all = is_all_scope(req);
    if all {
        if b.entities.is_empty() {
            return Some(Err(say(
                req,
                String::from("绘图上下文里没有实体 / no entities in the drawing context: 请先画一些对象"),
                String::from("No entities in the drawing context: draw something first"),
            )));
        }
        let handles: Vec<&str> = b.entities.iter().map(|e| e.handle.as_str()).collect();
        let script = format!("move {}", json!({ "handles": &handles, "delta": delta }));
        let reply = say(
            req,
            format!("移动 {} 个实体，位移 ({}, {})", handles.len(), num(delta[0]), num(delta[1])),
            format!("Moved {} entities by ({}, {})", handles.len(), num(delta[0]), num(delta[1])),
        );
        let reply = match b.entity_count {
            Some(total) if total > handles.len() => say(
                req,
                format!("{reply}（上下文只采样了前 {} 个 / sampled {}/{}）", handles.len(), handles.len(), total),
                format!("{reply} (sampled {}/{} entities)", handles.len(), total),
            ),
            _ => reply,
        };
        Some(Ok(draft(reply, script)))
    } else {
        if b.selection.is_empty() {
            return Some(Err(need_selection(req)));
        }
        let script = format!("move {}", json!({ "delta": delta }));
        let reply = say(
            req,
            format!("移动选中的 {} 个对象，位移 ({}, {})", b.selection.len(), num(delta[0]), num(delta[1])),
            format!("Moved the selection by ({}, {})", num(delta[0]), num(delta[1])),
        );
        Some(Ok(draft(reply, script)))
    }
}

/// `(dx, dy, direction word)`.
fn direction(req: &str) -> Option<(f64, f64, &'static str)> {
    if has_word(req, "up") || req.contains("向上") {
        Some((0.0, 1.0, "up"))
    } else if has_word(req, "down") || req.contains("向下") {
        Some((0.0, -1.0, "down"))
    } else if has_word(req, "left") || req.contains("向左") {
        Some((-1.0, 0.0, "left"))
    } else if has_word(req, "right") || req.contains("向右") {
        Some((1.0, 0.0, "right"))
    } else {
        None
    }
}

/// `把选中的旋转 45 度` / `rotate selection by 45`.
fn rule_rotate(req: &str, b: &ContextBrief) -> Option<Result<Draft, String>> {
    if !(req.contains("旋转") || has_word(req, "rotate")) {
        return None;
    }
    if b.selection.is_empty() && !is_all_scope(req) {
        return Some(Err(need_selection(req)));
    }
    let Some(mut angle) = number_after(req, &["旋转", "rotate", "角度", "by"]).or_else(|| first_number(req)) else {
        return Some(Err(say(
            req,
            String::from("请给出角度 / need an angle: 例 把选中的旋转 45 度"),
            String::from("Need an angle: try `rotate selection by 45`"),
        )));
    };
    if req.contains("顺时针") || has_word(req, "cw") || has_word(req, "clockwise") {
        angle = -angle;
    }
    let base = if req.contains("围绕") || req.contains("绕") || has_word(req, "around") { find_points(req).first().copied() } else { b.center() };
    let Some((cx, cy)) = base else {
        return Some(Err(say(
            req,
            String::from("上下文没有范围信息，无法确定旋转基点 / no extents in the context: 请给出基点，例 围绕 0,0 旋转 45 度"),
            String::from("No extents in the context: give a base point, e.g. `rotate selection around 0,0 by 45`"),
        )));
    };
    let handles = if is_all_scope(req) { b.entities.iter().map(|e| e.handle.clone()).collect::<Vec<_>>() } else { Vec::new() };
    let script = if handles.is_empty() {
        format!("rotate {}", json!({ "base": [cx, cy], "angle": angle }))
    } else {
        format!("rotate {}", json!({ "handles": handles, "base": [cx, cy], "angle": angle }))
    };
    let reply =
        say(req, format!("绕 ({}) 旋转 {}°", point_text(cx, cy), num(angle)), format!("Rotated {}° about ({})", num(angle), point_text(cx, cy)));
    Some(Ok(draft(reply, script)))
}

/// `左右镜像选中` / `上下镜像选中` / `mirror selection horizontally`.
fn rule_mirror(req: &str, b: &ContextBrief) -> Option<Result<Draft, String>> {
    if !(req.contains("镜像") || has_word(req, "mirror")) {
        return None;
    }
    if b.selection.is_empty() {
        return Some(Err(need_selection(req)));
    }
    let horizontal_flip = req.contains("左右")
        || has_word(req, "horizontal")
        || has_word(req, "horizontally")
        || has_word(req, "lr")
        || (has_word(req, "left") && has_word(req, "right"));
    let vertical_flip = req.contains("上下")
        || has_word(req, "vertical")
        || has_word(req, "vertically")
        || has_word(req, "tb")
        || (has_word(req, "up") && has_word(req, "down"));
    if horizontal_flip == vertical_flip {
        return Some(Err(say(
            req,
            String::from("请说明镜像方向 / need the axis: 例 左右镜像选中（竖轴）/ 上下镜像选中（横轴）"),
            String::from("Need the axis: try `mirror selection horizontally` or `mirror selection vertically`"),
        )));
    }
    let Some((x0, y0, x1, y1)) = b.extents else {
        return Some(Err(say(
            req,
            String::from("上下文没有范围信息，无法确定镜像轴 / no extents in the context"),
            String::from("No extents in the context, so the mirror axis is unknown"),
        )));
    };
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let pad = (x1 - x0).abs().max((y1 - y0).abs()).max(1.0);
    let (p1, p2) = if horizontal_flip { ([cx, y0 - pad], [cx, y1 + pad]) } else { ([x0 - pad, cy], [x1 + pad, cy]) };
    let script = format!("mirror {}", json!({ "p1": p1, "p2": p2 }));
    let reply = say(
        req,
        format!("{}镜像选中对象（生成副本）", if horizontal_flip { "左右" } else { "上下" }),
        format!("Mirrored the selection {} (a copy is kept)", if horizontal_flip { "left/right" } else { "up/down" }),
    );
    Some(Ok(draft(reply, script)))
}

/// `把选中的放大 2 倍` / `选中缩小 4 倍` / `scale selection by 1.5`.
fn rule_scale(req: &str, b: &ContextBrief) -> Option<Result<Draft, String>> {
    if !(req.contains("放大") || req.contains("缩小") || req.contains("缩放") || has_word(req, "scale")) {
        return None;
    }
    if b.selection.is_empty() && !is_all_scope(req) {
        return Some(Err(need_selection(req)));
    }
    let factor = if req.contains("缩小") {
        number_after(req, &["缩小"]).map(|n| 1.0 / n)
    } else if req.contains("放大") {
        number_after(req, &["放大"])
    } else {
        number_after(req, &["缩放", "scale", "by"])
    };
    let Some(factor) = factor else {
        return Some(Err(say(
            req,
            String::from("请给出倍数 / need the factor: 例 把选中的放大 2 倍; 选中缩小 4 倍"),
            String::from("Need the factor: try `scale selection by 2`"),
        )));
    };
    if !(factor > 0.0 && factor.is_finite()) {
        return Some(Err(out_of_range("倍数 / factor", req)));
    }
    let Some((cx, cy)) = b.center() else {
        return Some(Err(say(
            req,
            String::from("上下文没有范围信息，无法确定缩放基点 / no extents in the context"),
            String::from("No extents in the context, so the base point is unknown"),
        )));
    };
    let handles = if is_all_scope(req) { b.entities.iter().map(|e| e.handle.clone()).collect::<Vec<_>>() } else { Vec::new() };
    let script = if handles.is_empty() {
        format!("scale {}", json!({ "base": [cx, cy], "factor": factor }))
    } else {
        format!("scale {}", json!({ "handles": handles, "base": [cx, cy], "factor": factor }))
    };
    let reply = say(
        req,
        format!("以 ({}) 为基点缩放 {} 倍", point_text(cx, cy), num(factor)),
        format!("Scaled by {} about ({})", num(factor), point_text(cx, cy)),
    );
    Some(Ok(draft(reply, script)))
}

/// `把选中的圆向外偏移 5` / `offset the selected circle by 5`.
///
/// OFFSET needs a side point, and the offline context only carries geometry for
/// the sampled entities; a single selected circle is enough to choose the side.
/// Other shapes ask for the model provider instead of guessing.
fn rule_offset(req: &str, b: &ContextBrief) -> Option<Result<Draft, String>> {
    if !(req.contains("偏移") || has_word(req, "offset")) {
        return None;
    }
    let Some(distance) = number_after(req, &["偏移", "offset"]).or_else(|| first_number(req)) else {
        return Some(Err(say(
            req,
            String::from("请给出偏移距离 / need the distance: 例 把选中的圆向外偏移 5"),
            String::from("Need the distance: try `offset the selected circle by 5`"),
        )));
    };
    if !(distance > 0.0 && distance.is_finite()) {
        return Some(Err(out_of_range("距离 / distance", req)));
    }
    if b.selection.is_empty() {
        return Some(Err(need_selection(req)));
    }
    let circles = b.selected_circles();
    let Some(sample) = circles.first().copied() else {
        return Some(Err(say(
            req,
            String::from(
                "离线规则只支持单个选中圆的偏移 / offline offset supports one selected circle: \
请选中一个圆，或接入 OpenAI 兼容大模型（其他对象需要指定偏移侧）",
            ),
            String::from(
                "Offline offset supports one selected circle; select a circle or connect an OpenAI-compatible provider \
                (other objects need a side point)",
            ),
        )));
    };
    if circles.len() > 1 {
        return Some(Err(say(
            req,
            String::from("一次只能偏移一个圆 / offset one circle at a time: 请只选中一个圆"),
            String::from("Offset one circle at a time: select exactly one"),
        )));
    }
    let Some((cx, cy, r)) = sample.circle() else {
        return Some(Err(String::from("圆的几何信息不可用 / circle geometry unavailable in the context")));
    };
    let inward = req.contains("向内") || req.contains("内侧") || has_word(req, "inward") || has_word(req, "inside");
    if inward && distance >= r {
        return Some(Err(say(
            req,
            format!("向内偏移 {} 大于半径 {} / inward offset exceeds the radius", num(distance), num(r)),
            format!("Inward offset {} exceeds the radius {}", num(distance), num(r)),
        )));
    }
    let side = if inward { [cx, cy] } else { [cx + r + 1.0, cy] };
    let script = format!("offset {}", json!({ "handle": &sample.handle, "distance": distance, "side": side }));
    let reply = say(
        req,
        format!("把圆 {}（半径 {}）{}偏移 {}", sample.handle, num(r), if inward { "向内" } else { "向外" }, num(distance)),
        format!("Offset circle {} (radius {}) {} by {}", sample.handle, num(r), if inward { "inward" } else { "outward" }, num(distance)),
    );
    Some(Ok(draft(reply, script)))
}
