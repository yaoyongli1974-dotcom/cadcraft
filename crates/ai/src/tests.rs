//! Tests: the offline rule engine, draft application and the model-output parser.
//!
//! The rule tests read a fixed drawing-context block in the exact shape
//! `assistant::context` produces; the executor tests use a real `Session` and
//! verify through `drawing.inspect`, like the MCP integration tests do.

use serde_json::Value;
use serde_json::json;

use crate::assistant::Draft;
use crate::executor::{LineKind, apply, dry_run};
use crate::rules::offline_draft;
use cadcraft_engine::Session;

/// Two entities, one circle (`1A`) selected; extents centre is `(40, 20)`.
const CTX: &str = r#"DRAWING CONTEXT
title: test
entityCount: 2
counts: Circle=1 Line=1
extents: [[-10.0,-10.0],[90.0,50.0]]
layers: 0, Walls
currentLayer: 0
selection (1, handles): 1A
1A Circle [0] {"center":{"x":0.0,"y":0.0},"radius":10.0}
1B Line [0] {"p1":{"x":0.0,"y":0.0},"p2":{"x":1.0,"y":1.0}}
"#;

/// A drawing with no circles, for the "no target" branches.
const CTX_LINE_ONLY: &str = r#"DRAWING CONTEXT
entityCount: 1
extents: [[0.0,0.0],[10.0,10.0]]
layers: 0
selection (1, handles): 1B
1B Line [0] {"p1":{"x":0.0,"y":0.0},"p2":{"x":1.0,"y":1.0}}
"#;

/// One selected line, no circle.
const CTX_SELECTED_LINE: &str = r#"DRAWING CONTEXT
entityCount: 1
extents: [[0.0,0.0],[10.0,10.0]]
layers: 0, Walls
selection (1, handles): 1B
1B Line [0] {"p1":{"x":0.0,"y":0.0},"p2":{"x":1.0,"y":1.0}}
"#;

const CTX_NO_SELECTION: &str = r#"DRAWING CONTEXT
entityCount: 2
extents: [[-10.0,-10.0],[90.0,50.0]]
layers: 0, Walls
selection: (empty)
1A Circle [0] {"center":{"x":0.0,"y":0.0},"radius":10.0}
1B Line [0] {"p1":{"x":0.0,"y":0.0},"p2":{"x":1.0,"y":1.0}}
"#;

fn ok(req: &str, ctx: &str) -> Draft {
    match offline_draft(req, ctx) {
        Ok(d) => d,
        Err(e) => panic!("request {req:?} should succeed: {e}"),
    }
}

fn err(req: &str, ctx: &str) -> String {
    match offline_draft(req, ctx) {
        Ok(d) => panic!("request {req:?} should fail, got {d:?}"),
        Err(e) => e,
    }
}

/// The JSON argument of `command {json}`.
fn arg(draft: &Draft, command: &str) -> Value {
    let line = draft.script.trim_end_matches('\n');
    let Some(rest) = line.strip_prefix(command) else {
        panic!("expected {command} in {:?}", draft.script);
    };
    serde_json::from_str(rest.trim()).unwrap_or_else(|e| panic!("bad JSON in {:?}: {e}", draft.script))
}

// ---------------------------------------------------------------------------
// Rules: draw
// ---------------------------------------------------------------------------

#[test]
fn rule_line_chinese_and_english() {
    let d = ok("在 0,0 到 100,40 画线", CTX);
    assert_eq!(arg(&d, "line")["points"], json!([[0.0, 0.0], [100.0, 40.0]]));

    let d = ok("draw a line from (10,20) to 30,40", CTX);
    assert_eq!(arg(&d, "line")["points"], json!([[10.0, 20.0], [30.0, 40.0]]));

    let d = ok("line 0,0 5,5", CTX);
    assert_eq!(arg(&d, "line")["points"], json!([[0.0, 0.0], [5.0, 5.0]]));
}

#[test]
fn rule_line_asks_for_two_points() {
    let e = err("画线", CTX);
    assert!(e.contains("两个点"), "{e}");
    let e = err("line 10,20", CTX);
    assert!(e.contains("points"), "{e}");
}

#[test]
fn rule_rectangle_width_height_and_size_pair() {
    let d = ok("画宽 120 高 40 的矩形", CTX);
    assert_eq!(arg(&d, "rectang")["p1"], json!([0.0, 0.0]));
    assert_eq!(arg(&d, "rectang")["p2"], json!([120.0, 40.0]));

    let d = ok("rect 120x40 at 10,20", CTX);
    assert_eq!(arg(&d, "rectang")["p1"], json!([10.0, 20.0]));
    assert_eq!(arg(&d, "rectang")["p2"], json!([130.0, 60.0]));

    let d = ok("在 5,5 画宽 10 高 20 的长方形", CTX);
    assert_eq!(arg(&d, "rectang")["p2"], json!([15.0, 25.0]));
}

#[test]
fn rule_rectangle_needs_a_size() {
    let e = err("画宽 120 的矩形", CTX);
    assert!(e.contains("宽和高"), "{e}");
    let e = err("rect 0x40 at 0,0", CTX);
    assert!(e.contains("positive"), "{e}");
}

#[test]
fn rule_circle_both_phrasings() {
    let d = ok("在 100,100 画半径 60 的圆", CTX);
    assert_eq!(d.script.trim(), "circle 100,100 60");

    let d = ok("以 (10,20) 为圆心画半径 5 的圆", CTX);
    assert_eq!(d.script.trim(), "circle 10,20 5");

    let d = ok("circle 30 at 10,10", CTX);
    assert_eq!(d.script.trim(), "circle 10,10 30");
}

#[test]
fn rule_circle_rejects_bad_radius() {
    let e = err("在 0,0 画半径 0 的圆", CTX);
    assert!(e.contains("正数"), "{e}");
    let e = err("画圆", CTX);
    assert!(e.contains("圆心"), "{e}");
}

#[test]
fn rule_arc_center_radius_angles() {
    let d = ok("以 (0,0) 为圆心画半径 50 从 30 到 120 度的圆弧", CTX);
    let v = arg(&d, "arc");
    assert_eq!(v["center"], json!([0.0, 0.0]));
    assert_eq!(v["radius"], json!(50.0));
    assert_eq!(v["start"], json!(30.0));
    assert_eq!(v["end"], json!(120.0));

    let d = ok("arc center 10,10 radius 5 from 0 to 90", CTX);
    let v = arg(&d, "arc");
    assert_eq!(v["center"], json!([10.0, 10.0]));
    assert_eq!(v["start"], json!(0.0));
    assert_eq!(v["end"], json!(90.0));
}

#[test]
fn rule_arc_needs_angles() {
    let e = err("画圆弧", CTX);
    assert!(e.contains("圆心"), "{e}");
    let e = err("以 0,0 为圆心画半径 10 的圆弧", CTX);
    assert!(e.contains("起止角度"), "{e}");
}

#[test]
fn rule_text_quoted_and_plain() {
    let d = ok(r#"在 (10,20) 写 "你好""#, CTX);
    let v = arg(&d, "text");
    assert_eq!(v["at"], json!([10.0, 20.0]));
    assert_eq!(v["text"], json!("你好"));

    let d = ok("在 10,20 写 你好 世界", CTX);
    assert_eq!(arg(&d, "text")["text"], json!("你好 世界"));

    let d = ok(r#"text "Hello" at 10,20"#, CTX);
    assert_eq!(arg(&d, "text")["text"], json!("Hello"));

    let d = ok("write Plate at 0,0", CTX);
    assert_eq!(arg(&d, "text")["text"], json!("Plate"));
}

#[test]
fn rule_text_needs_content_and_point() {
    let e = err("在 10,20 写", CTX);
    assert!(e.contains("要写的文字"), "{e}");
    let e = err("写 你好", CTX);
    assert!(e.contains("文字位置"), "{e}");
}

// ---------------------------------------------------------------------------
// Rules: circles, layers
// ---------------------------------------------------------------------------

#[test]
fn rule_radius_change_largest_and_selected() {
    let d = ok("把最大的圆半径改成 25", CTX);
    let v = arg(&d, "properties.set");
    assert_eq!(v["handles"], json!(["1A"]));
    assert_eq!(v["radius"], json!(25.0));

    let d = ok("把选中的圆半径设为 15", CTX);
    assert_eq!(arg(&d, "properties.set")["radius"], json!(15.0));

    let d = ok("set the largest circle radius to 7", CTX);
    assert_eq!(arg(&d, "properties.set")["handles"], json!(["1A"]));
}

#[test]
fn rule_radius_change_needs_a_circle() {
    let e = err("把最大的圆半径改成 25", CTX_LINE_ONLY);
    assert!(e.contains("找不到要改的圆"), "{e}");
    let e = err("把最大的圆半径改成 0", CTX);
    assert!(e.contains("正数"), "{e}");
}

#[test]
fn rule_layer_new_colours_and_names() {
    let d = ok("新建红层 Walls", CTX);
    let v = arg(&d, "layer.new");
    assert_eq!(v["name"], json!("Walls"));
    assert_eq!(v["color"], json!("red"));

    let d = ok("新建绿色层 标注", CTX);
    let v = arg(&d, "layer.new");
    assert_eq!(v["name"], json!("标注"));
    assert_eq!(v["color"], json!("green"));

    let d = ok("new layer Hatch red", CTX);
    let v = arg(&d, "layer.new");
    assert_eq!(v["name"], json!("Hatch"));
    assert_eq!(v["color"], json!("red"));

    let d = ok("新建 Walls 层", CTX);
    assert_eq!(arg(&d, "layer.new")["name"], json!("Walls"));
}

#[test]
fn rule_layer_new_needs_a_name() {
    let e = err("新建层", CTX);
    assert!(e.contains("层名"), "{e}");
    let e = err("new layer red", CTX);
    assert!(e.contains("层名"), "{e}");
}

#[test]
fn rule_layer_switch() {
    let d = ok("切换到 Walls", CTX);
    assert_eq!(arg(&d, "layer.current")["name"], json!("Walls"));

    let d = ok("set current layer 0", CTX);
    assert_eq!(arg(&d, "layer.current")["name"], json!("0"));

    let d = ok("把当前层设为 Walls", CTX);
    assert_eq!(arg(&d, "layer.current")["name"], json!("Walls"));
}

#[test]
fn rule_layer_switch_rejects_unknown_layer() {
    let e = err("切换到 Missing", CTX);
    assert!(e.contains("不存在"), "{e}");
    assert!(e.contains("Walls"), "{e}");
}

#[test]
fn rule_layer_move_selection() {
    let d = ok("把选中的移到 Walls 层", CTX);
    let v = arg(&d, "laymch");
    assert_eq!(v["handles"], json!(["1A"]));
    assert_eq!(v["layer"], json!("Walls"));

    let d = ok("move selection to layer 0", CTX);
    assert_eq!(arg(&d, "laymch")["layer"], json!("0"));
}

#[test]
fn rule_layer_move_needs_selection() {
    let e = err("把选中的移到 Walls 层", CTX_NO_SELECTION);
    assert!(e.contains("没有选中"), "{e}");
}

// ---------------------------------------------------------------------------
// Rules: transforms
// ---------------------------------------------------------------------------

#[test]
fn rule_move_selection_and_all() {
    let d = ok("把选中的向上移动 10", CTX);
    let v = arg(&d, "move");
    assert_eq!(v["delta"], json!([0.0, 10.0]));
    assert!(v.get("handles").is_none(), "a selection move uses the live selection");

    let d = ok("move selection left 5", CTX);
    assert_eq!(arg(&d, "move")["delta"], json!([-5.0, 0.0]));

    let d = ok("把所有实体向下移动 7", CTX);
    let v = arg(&d, "move");
    assert_eq!(v["delta"], json!([0.0, -7.0]));
    assert_eq!(v["handles"], json!(["1A", "1B"]));

    let d = ok("move all right 3", CTX);
    assert_eq!(arg(&d, "move")["delta"], json!([3.0, 0.0]));
}

#[test]
fn rule_move_needs_direction_and_distance() {
    let e = err("把选中的移动 10", CTX);
    assert!(e.contains("方向"), "{e}");
    let e = err("把选中的向上移动", CTX);
    assert!(e.contains("距离"), "{e}");
    let e = err("把选中的向上移动 10", CTX_NO_SELECTION);
    assert!(e.contains("没有选中"), "{e}");
}

#[test]
fn rule_rotate_degrees_and_clockwise() {
    let d = ok("把选中的旋转 45 度", CTX);
    let v = arg(&d, "rotate");
    assert_eq!(v["base"], json!([40.0, 20.0]));
    assert_eq!(v["angle"], json!(45.0));

    let d = ok("rotate selection -30deg", CTX);
    assert_eq!(arg(&d, "rotate")["angle"], json!(-30.0));

    let d = ok("把选中的顺时针旋转 90 度", CTX);
    assert_eq!(arg(&d, "rotate")["angle"], json!(-90.0));

    let d = ok("围绕 (0,0) 旋转 15 度", CTX);
    assert_eq!(arg(&d, "rotate")["base"], json!([0.0, 0.0]));
}

#[test]
fn rule_rotate_needs_angle() {
    let e = err("把选中的旋转", CTX);
    assert!(e.contains("角度"), "{e}");
}

#[test]
fn rule_mirror_horizontal_and_vertical_axes() {
    let d = ok("左右镜像选中", CTX);
    let v = arg(&d, "mirror");
    assert_eq!(v["p1"][0], json!(40.0));
    assert_eq!(v["p2"][0], json!(40.0));
    assert_ne!(v["p1"][1], v["p2"][1]);

    let d = ok("把选中的上下镜像", CTX);
    let v = arg(&d, "mirror");
    assert_eq!(v["p1"][1], json!(20.0));
    assert_eq!(v["p2"][1], json!(20.0));
    assert_ne!(v["p1"][0], v["p2"][0]);
}

#[test]
fn rule_mirror_needs_axis() {
    let e = err("镜像选中", CTX);
    assert!(e.contains("镜像方向"), "{e}");
    let e = err("左右镜像选中", CTX_NO_SELECTION);
    assert!(e.contains("没有选中"), "{e}");
}

#[test]
fn rule_scale_enlarge_shrink_and_factor() {
    let d = ok("把选中的放大 2 倍", CTX);
    let v = arg(&d, "scale");
    assert_eq!(v["factor"], json!(2.0));
    assert_eq!(v["base"], json!([40.0, 20.0]));

    let d = ok("选中缩小 4 倍", CTX);
    assert_eq!(arg(&d, "scale")["factor"], json!(0.25));

    let d = ok("scale selection by 1.5", CTX);
    assert_eq!(arg(&d, "scale")["factor"], json!(1.5));
}

#[test]
fn rule_scale_rejects_zero() {
    let e = err("把选中的放大 0 倍", CTX);
    assert!(e.contains("正数"), "{e}");
    let e = err("把选中的放大", CTX);
    assert!(e.contains("倍数"), "{e}");
}

#[test]
fn rule_offset_single_selected_circle() {
    let d = ok("把选中的圆向外偏移 5", CTX);
    let v = arg(&d, "offset");
    assert_eq!(v["handle"], json!("1A"));
    assert_eq!(v["distance"], json!(5.0));
    assert!(v["side"][0].as_f64().unwrap_or(0.0) > 10.0, "outside the radius: {v}");

    let d = ok("选中向内偏移 2", CTX);
    let v = arg(&d, "offset");
    assert_eq!(v["side"], json!([0.0, 0.0]));
}

#[test]
fn rule_offset_rejects_other_shapes() {
    let e = err("把选中的向外偏移 5", CTX_SELECTED_LINE);
    assert!(e.contains("只支持单个选中圆"), "{e}");
    let e = err("选中向内偏移 20", CTX);
    assert!(e.contains("大于半径"), "{e}");
}

#[test]
fn unknown_request_mentions_examples() {
    let e = err("帮我设计一栋房子", CTX);
    assert!(e.contains("离线规则不认识"), "{e}");
    assert!(e.contains("画线"), "{e}");
    assert!(e.contains("OpenAI"), "{e}");
    assert!(err("", CTX).contains("输入为空"));
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

fn count(session: &mut Session, kind: &str) -> u64 {
    let v = session.execute("drawing.inspect", &json!({ "entities": false })).expect("inspect");
    v.pointer(&format!("/counts/{kind}")).and_then(Value::as_u64).unwrap_or(0)
}

#[test]
fn apply_creates_the_draft_as_one_undo_group() {
    let mut s = Session::new();
    let d = Draft {
        reply: String::new(),
        // JSON text form: TEXT's interactive prompt expects height and rotation, so the
        // three-token shorthand `text 0,20 Hi` cannot create text (see the report).
        script: String::from("circle 0,0 10\ntext {\"at\":[0,20],\"text\":\"Hi\"}\n"),
    };
    let report = apply(&mut s, &d).expect("apply");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.applied, 2);
    assert_eq!(count(&mut s, "Circle"), 1);
    assert_eq!(count(&mut s, "Text"), 1);

    // One undo group: a single U removes both.
    s.execute("undo", &json!({})).expect("undo");
    assert_eq!(count(&mut s, "Circle"), 0);
    assert_eq!(count(&mut s, "Text"), 0);
}

#[test]
fn apply_keeps_going_after_a_bad_line() {
    let mut s = Session::new();
    let d = Draft { reply: String::new(), script: String::from("circle 0,0 5\nbogusline 1,1\ncircle 20,20 3\n") };
    let report = apply(&mut s, &d).expect("apply");
    assert_eq!(report.applied, 2);
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(report.errors[0].contains("bogusline"), "{:?}", report.errors);
    assert!(!report.clean());
    assert_eq!(count(&mut s, "Circle"), 2);
}

#[test]
fn apply_rejects_an_empty_draft() {
    let mut s = Session::new();
    let d = Draft { reply: String::from("hello"), script: String::from("; only a comment\n\n") };
    assert!(apply(&mut s, &d).is_err());
}

#[test]
fn dry_run_names_commands_and_prompt_answers() {
    let d = Draft { reply: String::new(), script: String::from("line 0,0 10,0\npline 0,0 10,0 10,10\ncircle 0,0 5\nbogusline 1,1\n; comment\n") };
    assert_eq!(
        dry_run(&d),
        vec![
            LineKind::Command { command: String::from("line") },
            LineKind::Command { command: String::from("pline") },
            LineKind::Command { command: String::from("circle") },
            LineKind::Answer,
        ]
    );
}

#[test]
fn active_lines_drops_comments_and_blanks() {
    let d = Draft { reply: String::new(), script: String::from("; note\n\ncircle 0,0 5\n  \ntext 0,0 Hi\n") };
    assert_eq!(d.lines(), vec!["circle 0,0 5", "text 0,0 Hi"]);
}

// ---------------------------------------------------------------------------
// OpenAI provider: pure parser (no network)
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod openai_tests {
    use super::super::openai::{join_completions, parse_fenced, parse_json_object, parse_model_output, request_body, snippet, strip_choices};
    use crate::assistant::{ChatTurn, Draft, Role};
    use crate::provider::OpenAiSettings;
    use serde_json::json;

    #[test]
    fn parses_a_legal_json_draft() {
        let d = parse_model_output(r#"{"reply":"画好了","script":"circle 0,0 5"}"#).expect("draft");
        assert_eq!(d, Draft { reply: String::from("画好了"), script: String::from("circle 0,0 5") });
    }

    #[test]
    fn parses_json_with_prose_around_it() {
        let d = parse_model_output("Sure, here it is:\n{\"reply\":\"ok\",\"script\":\"circle 0,0 5\"}\nThanks!").expect("draft");
        assert_eq!(d.script, "circle 0,0 5");
        assert_eq!(d.reply, "ok");
        // Nested braces inside the JSON do not confuse the scan.
        let d = parse_model_output(r#"prefix {"reply":"ok","script":"text {\"at\":[0,0]}","extra":{"a":1}} suffix"#).expect("draft");
        assert!(d.script.starts_with("text "));
    }

    #[test]
    fn falls_back_to_a_fenced_block() {
        let raw = "Here is the script:\n```scr\ncircle 0,0 5\nline 0,0 10,0\n```\nDone.";
        let d = parse_model_output(raw).expect("draft");
        assert_eq!(d.reply, "Here is the script:");
        assert_eq!(d.script, "circle 0,0 5\nline 0,0 10,0");
        assert!(parse_fenced(raw).is_some());
        // A fence without a language line works too.
        assert_eq!(parse_fenced("```\ncircle 0,0 5\n```").map(|d| d.script), Some(String::from("circle 0,0 5")));
    }

    #[test]
    fn rejects_non_json_output() {
        let e = parse_model_output("I cannot help with that.").expect_err("must fail");
        assert!(e.contains("unexpected model output"), "{e}");
        assert!(parse_json_object("no object here").is_none());
        assert!(parse_fenced("no fence here").is_none());
    }

    #[test]
    fn unwraps_choices_and_endpoint_errors() {
        let raw = json!({"choices":[{"message":{"content":" {\"reply\":\"a\",\"script\":\"b\"}"}}]}).to_string();
        assert_eq!(strip_choices(&raw).expect("content"), "{\"reply\":\"a\",\"script\":\"b\"}");

        let raw = json!({"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}).to_string();
        let e = strip_choices(&raw).expect_err("endpoint error");
        assert!(e.contains("Incorrect API key provided"), "{e}");

        let long = "x".repeat(1000);
        let raw = json!({"error":{"message":long}}).to_string();
        let e = strip_choices(&raw).expect_err("endpoint error");
        assert!(e.chars().count() <= 330, "truncated error: {} chars", e.chars().count());
    }

    #[test]
    fn joins_the_endpoint_once() {
        assert_eq!(join_completions("https://api.openai.com/v1/"), "https://api.openai.com/v1/chat/completions");
        assert_eq!(join_completions("http://127.0.0.1:11434/v1"), "http://127.0.0.1:11434/v1/chat/completions");
        assert_eq!(join_completions("https://x/v1/chat/completions"), "https://x/v1/chat/completions");
    }

    #[test]
    fn request_body_keeps_the_key_out() {
        let settings = OpenAiSettings { api_key: String::from("sk-secret"), model: String::from("m"), ..Default::default() };
        let history = vec![ChatTurn { role: Role::User, text: String::from("hi") }, ChatTurn { role: Role::Assistant, text: String::from("hello") }];
        let body = request_body(&settings, "DRAWING CONTEXT", &history, "draw a line", &[], &[]);
        let text = serde_json::to_string(&body).expect("json");
        assert!(!text.contains("sk-secret"), "the API key never goes in the body");
        assert_eq!(body["response_format"]["type"], json!("json_object"));
        assert_eq!(body["messages"].as_array().map(|a| a.len()), Some(5));
    }

    #[test]
    fn snippets_count_characters() {
        assert_eq!(snippet("  你好世界  ", 2), "你好…");
        assert_eq!(snippet("abc", 10), "abc");
    }
}

/// The strongest check that the rule engine speaks CADCraft: every absolute-coordinate
/// draft it produces must run on a real session without a single failed line.
mod offline_apply {
    use super::*;

    #[test]
    fn absolute_rule_drafts_apply_cleanly() {
        let requests = [
            "在 0,0 到 100,40 画线",
            "画宽 120 高 40 的矩形",
            "以 (50,50) 为圆心画半径 20 的圆",
            "以 (0,0) 为圆心画半径 50 从 30 到 120 的圆弧",
            "在 10,20 写 你好",
            "新建红层 Walls",
        ];
        for request in requests {
            let draft = offline_draft(request, "").unwrap_or_default();
            assert!(!draft.script.trim().is_empty(), "{request}: no script (reply: {})", draft.reply);
            let mut session = Session::new();
            let report = apply(&mut session, &draft).unwrap_or_default();
            assert!(report.errors.is_empty(), "{request}: {:?} in {:?}", report.errors, draft.script);
            assert!(report.applied >= 1, "{request}: {:?}", draft.script);
        }
    }
}

// ---------------------------------------------------------------------------
// Reference images: staging, request parts and the offline notice
// ---------------------------------------------------------------------------

mod images_tests {
    use super::*;
    use base64::Engine as _;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::new_rgba8(width, height);
        let mut out = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).expect("encode png");
        out
    }

    fn staged() -> crate::images::StagedImage {
        crate::images::StagedImage { name: String::from("view.png"), mime: String::from("image/png"), data_b64: String::new(), width: 10, height: 10 }
    }

    #[test]
    fn stages_a_small_png_as_is() {
        let st = crate::images::stage_bytes("view.png", &png_bytes(8, 5)).expect("stage");
        assert_eq!(st.mime, "image/png");
        assert_eq!((st.width, st.height), (8, 5));
        let bytes = base64::engine::general_purpose::STANDARD.decode(st.data_b64).expect("b64");
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']), "lossless png kept");
    }

    #[test]
    fn non_images_are_refused_with_a_hint() {
        let e = crate::images::stage_bytes("notes.txt", b"not an image").expect_err("refused");
        assert!(e.contains("PNG, JPEG, BMP and TIFF"), "{e}");
        let e = crate::images::stage_bytes("empty.png", &[]).expect_err("refused");
        assert!(e.contains("empty"), "{e}");
        assert!(crate::images::is_image_ext("JPG").is_some());
        assert!(crate::images::is_image_ext("gif").is_none(), "formats match io::raster");
    }

    #[test]
    fn oversized_sides_are_downscaled() {
        let st = crate::images::stage_bytes("wide.png", &png_bytes(2400, 8)).expect("stage");
        assert!(st.width <= crate::images::MAX_SIDE, "{}", st.width);
        assert!(st.height >= 1, "{}", st.height);
    }

    #[test]
    fn the_user_message_becomes_parts_only_with_images() {
        let text = crate::images::user_content("以选中的为准", &[], &[]);
        assert_eq!(text, serde_json::json!("以选中的为准"));
        let value = crate::images::user_content("以图为准", &[staged()], &[]);
        let parts = value.as_array().expect("parts");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], serde_json::json!({ "type": "text", "text": "以图为准" }));
        let url = parts[1]["image_url"]["url"].as_str().expect("url");
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
    }

    #[test]
    fn offline_with_an_image_says_so_instead_of_failing() {
        let settings = crate::provider::AiSettings::default();
        let draft =
            crate::provider::generate(&settings, "", &[], "以 0,0 为圆心画半径 30 的圆", std::slice::from_ref(&staged()), &[]).expect("draft");
        assert!(draft.reply.starts_with("The offline engine sees no pixels."), "{}", draft.reply);
        assert_eq!(draft.script.trim(), "circle 0,0 30");
    }

    #[test]
    fn the_image_cap_is_enforced() {
        let settings = crate::provider::AiSettings::default();
        let images: Vec<_> = (0..crate::images::MAX_IMAGES + 1).map(|_| staged()).collect();
        let e = crate::provider::generate(&settings, "", &[], "画圆", &images, &[]).expect_err("capped");
        assert!(e.contains("At most"), "{e}");
        let e = crate::assistant::suggest(&mut Session::new(), &settings, &[], "画圆", &images, &[]).expect_err("capped");
        assert!(e.contains("At most"), "{e}");
    }
}

/// Reference documents: staging, the block that rides the question, and the offline reply.
mod document_tests {
    use crate::documents::{MAX_DOCUMENTS, StagedDocument};
    use crate::images::StagedImage;
    use crate::provider::AiSettings;

    fn doc(name: &str, text: &str) -> StagedDocument {
        StagedDocument::stage(name, text.as_bytes()).expect("stage")
    }

    #[test]
    fn a_document_is_text_ahead_of_the_question() {
        let value = crate::images::user_content("画一面墙", &[], &[doc("spec.md", "Wall height 2400 mm")]);
        let text = value.as_str().expect("plain text content");
        assert!(text.starts_with("REFERENCE DOCUMENTS\n<document name=\"spec.md\" kind=\"text\">"), "{text}");
        assert!(text.contains("Wall height 2400 mm"));
        assert!(text.ends_with("画一面墙"));
    }

    #[test]
    fn documents_and_images_share_the_content_parts() {
        let image = StagedImage { name: String::from("view"), mime: String::from("image/png"), data_b64: String::from("AAAA"), width: 1, height: 1 };
        let value = crate::images::user_content("看这两份", &[image], &[doc("a.txt", "first")]);
        let parts = value.as_array().expect("parts");
        assert_eq!(parts.len(), 2);
        assert!(parts[0]["text"].as_str().is_some_and(|t| t.contains("first") && t.ends_with("看这两份")));
        assert_eq!(parts[1]["type"], serde_json::json!("image_url"));
    }

    #[test]
    fn offline_asks_with_documents_say_they_cannot_read_them() {
        let settings = AiSettings::default();
        let draft = crate::provider::generate(&settings, "", &[], "以 0,0 为圆心画半径 5 的圆", &[], &[doc("a.txt", "notes")]).expect("draft");
        assert!(draft.reply.starts_with("The offline engine cannot read documents."), "{}", draft.reply);
        assert!(draft.script.contains("circle"), "{}", draft.script);
    }

    #[test]
    fn too_many_documents_are_refused_before_the_provider() {
        let settings = AiSettings::default();
        let docs: Vec<_> = (0..=MAX_DOCUMENTS).map(|i| doc(&format!("{i}.txt"), "text")).collect();
        let err = crate::provider::generate(&settings, "", &[], "画圆", &[], &docs).expect_err("capped");
        assert!(err.contains("documents"), "{err}");
    }
}
