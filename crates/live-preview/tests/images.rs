//! Images and embeds through the Document facade: away from the caret an
//! image is one object in the text (`U+FFFC`, as Kit draws it) and a note
//! embed is a link to the note; with the caret on either, its source shows.
//! Expected values are worked out by hand.

use live_preview::Document;
use live_preview::conceal::{Image, OBJECT, Target};

#[test]
fn an_image_embed_is_one_object_until_the_caret_is_on_it() {
    let src = "See ![[cat.png]] here\n\nEnd";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    let view = &d.views()[0];
    assert_eq!(view.display_text(), format!("See {OBJECT} here"));
    assert_eq!(
        view.images(),
        [Image {
            at: 4,
            target: Target::Note("cat.png".into()),
            alt: String::new(),
            width: None,
            height: None,
        }]
    );
    d.set_caret(src.find("cat").unwrap());
    assert_eq!(d.views()[0].display_text(), "See ![[cat.png]] here");
    assert!(d.views()[0].images().is_empty());
    // Away again, it's drawn again.
    d.move_doc_end(false);
    assert_eq!(d.views()[0].images().len(), 1);
}

#[test]
fn a_markdown_image_and_a_sized_embed() {
    let src =
        "![a cat](pics/cat%201.jpg) ![[dog.webp|300]] ![[owl.gif|40x30]] ![[bat.png|night]]\n\nEnd";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    let images = d.views()[0].images().to_vec();
    let sizes: Vec<_> = images
        .iter()
        .map(|i| (i.target.clone(), i.alt.clone(), i.width, i.height))
        .collect();
    assert_eq!(
        sizes,
        [
            (
                Target::Note("pics/cat 1.jpg".into()),
                "a cat".into(),
                None,
                None
            ),
            (
                Target::Note("dog.webp".into()),
                String::new(),
                Some(300),
                None
            ),
            (
                Target::Note("owl.gif".into()),
                String::new(),
                Some(40),
                Some(30)
            ),
            (Target::Note("bat.png".into()), "night".into(), None, None),
        ]
    );
    let objects = format!("{OBJECT} {OBJECT} {OBJECT} {OBJECT}");
    assert_eq!(d.views()[0].display_text(), objects);
}

#[test]
fn a_note_embed_is_a_link_to_the_note() {
    let src = "![[Recipes#Soup|the soup]] and ![[Recipes]]\n\nEnd";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    let view = &d.views()[0];
    assert_eq!(view.display_text(), "the soup and Recipes");
    let targets: Vec<_> = view.links().iter().map(|l| l.target.clone()).collect();
    assert_eq!(
        targets,
        [
            Target::Note("Recipes#Soup".into()),
            Target::Note("Recipes".into())
        ]
    );
    assert!(view.images().is_empty());
}

#[test]
fn an_image_needs_no_alt_text_and_a_note_shown_as_one_is_a_link() {
    let src = "![](dog.png) ![the plan](Orbit%20roadmap.md)\n\nEnd";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    let view = &d.views()[0];
    assert_eq!(view.display_text(), format!("{OBJECT} the plan"));
    assert_eq!(view.images()[0].target, Target::Note("dog.png".into()));
    assert_eq!(
        view.links()[0].target,
        Target::Note("Orbit roadmap.md".into())
    );
}

#[test]
fn an_image_in_a_link_goes_where_the_link_does() {
    let src = "[![a](cat.png)](https://cats.example)\n\nEnd";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    let view = &d.views()[0];
    assert_eq!(view.display_text(), OBJECT);
    assert_eq!(view.links()[0].range, 0..OBJECT.len());
    assert_eq!(
        view.links()[0].target,
        Target::Url("https://cats.example".into())
    );
}
