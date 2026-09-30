from pixelplus_tts.pronounce import (apply_pronunciations, builtin_pronunciations, compile_rules, merge,
                                    parse_pronunciations_txt, to_phonemes)


def apply(pairs, text):
    return apply_pronunciations(text, compile_rules(pairs))


def test_whole_word_only():
    assert apply([("TSO", "T S O")], "TSO rocks, TSOs don't")[0] == "T S O rocks, TSOs don't"
    assert apply([("Sia", "See-a")], "Asia and Sia")[0] == "Asia and See-a"


def test_capitalized_entries_are_case_sensitive():
    rules = [("LED", "L E D")]
    assert apply(rules, "LED lights; she led the way")[0] == "L E D lights; she led the way"


def test_lowercase_entries_match_any_case():
    assert apply([("xmas", "Christmas")], "XMAS and Xmas and xmas")[0] == "Christmas and Christmas and Christmas"


def test_longest_phrase_first():
    rules = [("Navidad", "/nˌɑvidˈɑd/"), ("Feliz Navidad", "/fəlˈiz nˌɑvidˈɑd/")]
    text, ipa = apply(rules, "Feliz Navidad!")
    assert ipa == ["fəlˈiz nˌɑvidˈɑd"]
    assert text == "\x000\x00!"


def test_punctuation_edges_and_unicode():
    text, ipa = apply([("Noël", "/noʊˈɛl/"), ("St. Nick", "Saint Nick")], "Joyeux Noël, St. Nick.")
    assert text == "Joyeux \x000\x00, Saint Nick."
    assert ipa == ["noʊˈɛl"]
    # "w/" and "&" are whole "words" too
    assert apply([("w/", "with"), ("&", "and")], "cocoa w/ marshmallows & cream")[0] == "cocoa with marshmallows and cream"


def test_ipa_skipped_when_absent():
    text, ipa = apply([("Noel", "/noʊˈɛl/")], "no match here")
    assert ipa == [] and text == "no match here"


def test_to_phonemes_splices_ipa():
    rules = compile_rules([("Noel", "/noʊˈɛl/"), ("TSO", "T S O")])
    got = to_phonemes("The first Noel by TSO", rules, lambda s: f"<{s.strip()}>")
    assert got == "<The first> noʊˈɛl <by T S O>"


def test_parse_txt_and_builtins():
    pairs = parse_pronunciations_txt("# comment\n#\n#1 = number one\nFoo = Bar  \n\nbad line\n")
    assert pairs == [("#1", "number one"), ("Foo", "Bar")]
    builtin = dict(builtin_pronunciations())
    assert builtin["Noel"] == "/noʊˈɛl/"
    assert builtin["TSO"] == "T S O"
    assert len(builtin) > 90


def test_user_entries_override_builtins():
    merged = dict(merge([("Noel", "/noʊˈɛl/")], [{"word": "Noel", "say": "No well"}, {"word": "Griswold", "say": "Griz wold"}]))
    assert merged == {"Noel": "No well", "Griswold": "Griz wold"}
