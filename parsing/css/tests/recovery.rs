use brows12_css::Stylesheet;

#[test]
fn sheet_with_light_dark_parses() {
    let css = "html{color-scheme:light dark;background:light-dark(#eee,#222)}body{font:16px/1.6 system-ui,sans-serif;max-width:26em;margin:auto;padding:25vh 2em 2em;text-align:center}";
    let sheet = Stylesheet::parse(css, brows12_css::Origin::Author);
    assert!(sheet.is_ok(), "example.com sheet must parse: {:?}", sheet.err());
    assert_eq!(sheet.unwrap().rules.len(), 2, "both rules survive");
}

#[test]
fn unknown_at_rule_does_not_kill_sheet() {
    let sheet =
        Stylesheet::parse("@unknown-rule x{}; p { color: red }", brows12_css::Origin::Author);
    assert!(sheet.is_ok());
}
