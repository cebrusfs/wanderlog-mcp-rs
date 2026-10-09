use super::*;
use serde_json::json;

#[test]
fn list_and_object_components() {
    let mut doc = json!({"a": [1, 2, 3], "o": {"k": "v"}});
    apply(&mut doc, &json!({"p": ["a", 1], "li": 9})).unwrap();
    assert_eq!(doc["a"], json!([1, 9, 2, 3]));
    apply(&mut doc, &json!({"p": ["a", 0], "ld": 1})).unwrap();
    assert_eq!(doc["a"], json!([9, 2, 3]));
    apply(&mut doc, &json!({"p": ["a", 2], "lm": 0})).unwrap();
    assert_eq!(doc["a"], json!([3, 9, 2]));
    apply(&mut doc, &json!({"p": ["a", 1], "ld": 9, "li": 7})).unwrap();
    assert_eq!(doc["a"], json!([3, 7, 2]));
    apply(&mut doc, &json!({"p": ["o", "k"], "od": "v", "oi": "w"})).unwrap();
    apply(&mut doc, &json!({"p": ["o", "n"], "od": null, "oi": null})).unwrap();
    assert_eq!(doc["o"], json!({"k": "w", "n": null}));
    apply(&mut doc, &json!({"p": ["o", "k"], "od": "w"})).unwrap();
    assert_eq!(doc["o"], json!({"n": null}));
    assert!(apply(&mut doc, &json!({"p": ["a", 9], "ld": 1})).is_err());
}

#[test]
fn text0_counts_utf16_units() {
    let mut doc = json!({"title": "Trip to Tokyo 🗼"});
    let op = json!({"p": ["title"], "t": "text0", "o": [
            {"p": 0, "d": "Trip to Tokyo 🗼"}, {"p": 0, "i": "東京 🗼 plan"}]});
    apply(&mut doc, &op).unwrap();
    assert_eq!(doc["title"], "東京 🗼 plan");
    let bad = json!({"p": ["title"], "t": "text0", "o": [{"p": 0, "d": "nope"}]});
    assert!(apply(&mut doc, &bad).is_err());
}

#[test]
fn rich_text_insert_retain_delete() {
    let mut doc = json!({"text": {"ops": [{"insert": "\n"}]}});
    let p = json!(["text"]);
    apply(
        &mut doc,
        &json!({"p": p, "t": "rich-text", "o": [{"insert": "Go at sunset."}]}),
    )
    .unwrap();
    assert_eq!(doc["text"], json!({"ops": [{"insert": "Go at sunset.\n"}]}));
    let append = json!([{"retain": 13}, {"insert": " Bring a camera."}]);
    apply(&mut doc, &json!({"p": p, "t": "rich-text", "o": append})).unwrap();
    assert_eq!(
        doc["text"]["ops"][0]["insert"],
        "Go at sunset. Bring a camera.\n"
    );
    let replace = json!([{"insert": "新しい"}, {"delete": 29}]);
    apply(&mut doc, &json!({"p": p, "t": "rich-text", "o": replace})).unwrap();
    assert_eq!(doc["text"], json!({"ops": [{"insert": "新しい\n"}]}));
}

#[test]
fn rich_text_keeps_formatting_runs() {
    let mut doc = json!({"ops": [
            {"insert": "see "}, {"insert": "site", "attributes": {"link": "https://x"}}, {"insert": "\n"}]});
    apply_rich_text(&mut doc, &json!([{"retain": 8}, {"insert": "!"}])).unwrap();
    assert_eq!(doc["ops"].as_array().unwrap().len(), 3);
    assert_eq!(doc["ops"][2]["insert"], "!\n");
    assert_eq!(rich_text_doc("hi"), json!({"ops": [{"insert": "hi\n"}]}));
}
#[test]
fn malformed_components_and_subtypes_reject_without_partial_document_writes() {
    let original =
        serde_json::json!({"a":[1,2],"o":{},"s":"😀x","rich":{"ops":[{"insert":"abc"}]}});
    for op in [
        json!({}),
        json!({"p":[]}),
        json!({"p":[true]}),
        json!({"p":["missing",0],"li":1}),
        json!({"p":["a",-1],"li":1}),
        json!({"p":["a",9],"li":1}),
        json!({"p":["s",0],"li":1}),
        json!({"p":["o","x"],"lm":1}),
        json!({"p":["a",0],"lm":"x"}),
        json!({"p":["a",0],"lm":9}),
        json!({"p":["a",0],"oi":1}),
        json!({"p":["o",0],"oi":1}),
        json!({"p":["s"],"t":"unknown","o":[]}),
        json!({"p":["s"],"t":"text0"}),
        json!({"p":["a"],"t":"text0","o":[]}),
        json!({"p":["s"],"t":"text0","o":{}}),
        json!({"p":["s"],"t":"text0","o":[{}]}),
        json!({"p":["s"],"t":"text0","o":[{"p":99,"i":"x"}]}),
        json!({"p":["s"],"t":"text0","o":[{"p":0}]}),
        json!({"p":["s"],"t":"text0","o":[{"p":1,"i":"x"}]}),
        json!({"p":["s"],"t":"rich-text","o":[]}),
        json!({"p":["rich"],"t":"rich-text","o":{}}),
        json!({"p":["rich"],"t":"rich-text","o":[{"retain":1,"attributes":{}}]}),
        json!({"p":["rich"],"t":"rich-text","o":[{"delete":99}]}),
        json!({"p":["rich"],"t":"rich-text","o":[{}]}),
        json!({"p":["a",0]}),
    ] {
        let mut doc = original.clone();
        assert!(apply(&mut doc, &op).is_err(), "{op}");
        assert_eq!(doc, original, "{op}");
    }
    let mut doc = json!({"ops":[{"insert":{"image":"x"}},{"insert":"abc","attributes":{"bold":true}},{"insert":""}]});
    apply_rich_text(&mut doc, &json!([{"retain":1},{"delete":1},{"insert":"z"}])).unwrap();
    assert_eq!(
        doc["ops"],
        json!([{"insert":{"image":"x"}},{"insert":"z"},{"insert":"bc","attributes":{"bold":true}}])
    );
}
