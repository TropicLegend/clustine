use super::*;
use clustine_data::{blocks, items};
use clustine_world::components::{
    Component, Components, EffectInstance, FireworkExplosion, FireworkShape, Fireworks,
    PotionContents, structure,
};

use crate::region::block_entities::nbt_put_string;
use crate::region::containers::{container_data, read_container};

/// The game's `NbtOps` form of 25 component values as its SNBT prints them
/// (`/home/claude/reference/n9-hashes/answers.tsv`, made by `HashAnswers.java` beside
/// it from the game's own codecs): each line's first word is the component's kind.
const ANSWERS: &[(&str, &str)] = &[
    (
        "potion_contents swiftness",
        r#"{potion:"minecraft:swiftness"}"#,
    ),
    ("potion_contents empty", "{}"),
    (
        "potion_contents water colour 0xFF0000 two effects named",
        r#"{custom_color:16711680,custom_effects:[{amplifier:1b,duration:200,id:"minecraft:speed",show_icon:1b},{ambient:1b,duration:100,id:"minecraft:poison",show_icon:1b,show_particles:0b}],custom_name:"fancy",potion:"minecraft:water"}"#,
    ),
    ("potion_duration_scale 0.25", "0.25f"),
    ("damage 5", "5"),
    ("damage 0", "0"),
    ("intangible_projectile", "{}"),
    ("rarity epic", r#""epic""#),
    ("rarity common", r#""common""#),
    ("item_name literal Hi", r#""Hi""#),
    (
        "item_name translatable item.minecraft.apple",
        r#"{translate:"item.minecraft.apple"}"#,
    ),
    ("use_cooldown 1", "{seconds:1.0f}"),
    (
        "use_cooldown 2.5 group minecraft:ender_pearl",
        r#"{cooldown_group:"minecraft:ender_pearl",seconds:2.5f}"#,
    ),
    ("ominous_bottle_amplifier 2", "2"),
    (
        "suspicious_stew_effects night_vision 100",
        r#"[{duration:100,id:"minecraft:night_vision"}]"#,
    ),
    ("fireworks flight 1 none", "{flight_duration:1b}"),
    (
        "fireworks flight 2 star [red] fade [blue] trail",
        r#"{explosions:[{colors:[16711680],fade_colors:[255],has_trail:1b,shape:"star"}],flight_duration:2b}"#,
    ),
    (
        "tooltip_display hide false [enchantments, damage]",
        r#"{hidden_components:["minecraft:enchantments","minecraft:damage"]}"#,
    ),
    ("tooltip_display hide true []", "{hide_tooltip:1b}"),
    (
        "enchantments sharpness 3 unbreaking 1",
        r#"{"minecraft:sharpness":3,"minecraft:unbreaking":1}"#,
    ),
    ("enchantments empty", "{}"),
    (
        "banner_patterns stripe_bottom red, cross white",
        r#"[{color:"red",pattern:"minecraft:stripe_bottom"},{color:"white",pattern:"minecraft:cross"}]"#,
    ),
    ("charged_projectiles empty", "[]"),
    ("charged_projectiles arrow", r#"[{id:"minecraft:arrow"}]"#),
    (
        "charged_projectiles tipped arrow poison, rocket",
        r#"[{components:{"minecraft:potion_contents":{potion:"minecraft:poison"}},id:"minecraft:tipped_arrow"},{id:"minecraft:firework_rocket"}]"#,
    ),
];

/// A reader of the SNBT the answers are in: compounds, lists, quoted strings, and
/// numbers with the game's suffixes (none for an int).
struct Snbt<'a> {
    text: &'a [u8],
    at: usize,
}

impl Snbt<'_> {
    fn read(text: &str) -> Value {
        let mut reader = Snbt {
            text: text.as_bytes(),
            at: 0,
        };
        let value = reader.value();
        assert_eq!(reader.at, text.len(), "all of {text} read");
        value
    }

    fn peek(&self) -> u8 {
        self.text[self.at]
    }

    fn expect(&mut self, byte: u8) {
        assert_eq!(self.peek(), byte, "at {} of the SNBT", self.at);
        self.at += 1;
    }

    fn value(&mut self) -> Value {
        match self.peek() {
            b'{' => {
                self.at += 1;
                let mut entries = Vec::new();
                while self.peek() != b'}' {
                    let key = if self.peek() == b'"' {
                        self.quoted()
                    } else {
                        self.word()
                    };
                    self.expect(b':');
                    entries.push((key, self.value()));
                    if self.peek() == b',' {
                        self.at += 1;
                    }
                }
                self.at += 1;
                Value::Map(entries)
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                while self.peek() != b']' {
                    items.push(self.value());
                    if self.peek() == b',' {
                        self.at += 1;
                    }
                }
                self.at += 1;
                Value::List(items)
            }
            b'"' => Value::String(self.quoted()),
            _ => {
                let word = self.word();
                let (number, suffix) = word.split_at(
                    word.find(|c: char| c.is_ascii_alphabetic())
                        .unwrap_or(word.len()),
                );
                match suffix {
                    "b" => Value::Byte(number.parse().expect("a byte")),
                    "s" => Value::Short(number.parse().expect("a short")),
                    "L" => Value::Long(number.parse().expect("a long")),
                    "f" => Value::Float(number.parse().expect("a float")),
                    "d" => Value::Double(number.parse().expect("a double")),
                    "" if number.contains('.') => Value::Double(number.parse().expect("a double")),
                    "" => Value::Int(number.parse().expect("an int")),
                    _ => panic!("no number: {word}"),
                }
            }
        }
    }

    fn word(&mut self) -> String {
        let start = self.at;
        while self.at < self.text.len() && !b"{}[],:\"".contains(&self.peek()) {
            self.at += 1;
        }
        String::from_utf8(self.text[start..self.at].to_vec()).expect("ASCII")
    }

    fn quoted(&mut self) -> String {
        self.expect(b'"');
        let start = self.at;
        while self.peek() != b'"' {
            self.at += 1;
        }
        let text = String::from_utf8(self.text[start..self.at].to_vec()).expect("ASCII");
        self.at += 1;
        text
    }
}

/// The game's SNBT of the tag `tag` whose payload begins `data`, as `StringTagVisitor`
/// prints it: a compound's keys sorted, a key quoted where it is not a plain word, and
/// each number with its type's suffix. Read from the bytes, so that it shows the tag
/// types written, and arrays apart from lists.
fn snbt(tag: u8, data: &[u8]) -> (String, &[u8]) {
    let mut out = String::new();
    let rest = print(tag, data, &mut out);
    (out, rest)
}

fn print<'a>(tag: u8, data: &'a [u8], out: &mut String) -> &'a [u8] {
    let int = |data: &[u8]| i32::from_be_bytes(data[..4].try_into().expect("four bytes"));
    match tag {
        TAG_BYTE => {
            out.push_str(&format!("{}b", data[0] as i8));
            &data[1..]
        }
        TAG_SHORT => {
            out.push_str(&format!("{}s", i16::from_be_bytes([data[0], data[1]])));
            &data[2..]
        }
        TAG_INT => {
            out.push_str(&int(data).to_string());
            &data[4..]
        }
        TAG_LONG => {
            let n = i64::from_be_bytes(data[..8].try_into().expect("eight bytes"));
            out.push_str(&format!("{n}L"));
            &data[8..]
        }
        TAG_FLOAT => {
            let n = f32::from_be_bytes(data[..4].try_into().expect("four bytes"));
            out.push_str(&format!("{n:?}f"));
            &data[4..]
        }
        TAG_DOUBLE => {
            let n = f64::from_be_bytes(data[..8].try_into().expect("eight bytes"));
            out.push_str(&format!("{n:?}d"));
            &data[8..]
        }
        TAG_STRING => {
            let (text, rest) = nbt_string(data).expect("a string");
            out.push('"');
            out.push_str(std::str::from_utf8(text).expect("UTF-8"));
            out.push('"');
            rest
        }
        TAG_INT_ARRAY => {
            let count = usize::try_from(int(data)).expect("a length");
            let items: Vec<String> = (0..count)
                .map(|i| int(&data[4 + 4 * i..]).to_string())
                .collect();
            out.push_str(&format!("[I;{}]", items.join(",")));
            &data[4 + 4 * count..]
        }
        TAG_LIST => {
            let element = data[0];
            let count = int(&data[1..]);
            let mut rest = &data[5..];
            out.push('[');
            for i in 0..count {
                if i > 0 {
                    out.push(',');
                }
                rest = print(element, rest, out);
            }
            out.push(']');
            rest
        }
        TAG_COMPOUND => {
            let mut entries = Vec::new();
            let mut rest = data;
            while rest[0] != TAG_END {
                let inner = rest[0];
                let (name, after) = nbt_string(&rest[1..]).expect("a name");
                let (value, after) = snbt(inner, after);
                entries.push((String::from_utf8(name.to_vec()).expect("UTF-8"), value));
                rest = after;
            }
            entries.sort();
            let plain = |key: &str| {
                !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
            };
            let shown: Vec<String> = entries
                .into_iter()
                .map(|(key, value)| {
                    if plain(&key) {
                        format!("{key}:{value}")
                    } else {
                        format!("\"{key}\":{value}")
                    }
                })
                .collect();
            out.push_str(&format!("{{{}}}", shown.join(",")));
            &rest[1..]
        }
        _ => panic!("no tag {tag}"),
    }
}

fn written(value: &Value) -> (u8, Vec<u8>) {
    let mut payload = Vec::new();
    put_value(&mut payload, value);
    (tag_of(value), payload)
}

#[test]
fn each_answer_of_the_game_is_written_as_its_nbt_and_read_back() {
    for (what, game) in ANSWERS {
        let kind_name = format!("minecraft:{}", what.split(' ').next().expect("a word"));
        let kind = Kind::named(&kind_name).expect("a kind Clustine knows");
        let component = component_from(kind, &Snbt::read(game))
            .unwrap_or_else(|error| panic!("{what} reads: {error}"));

        let (tag, payload) = written(&structure(&component));
        let (shown, rest) = snbt(tag, &payload);
        assert_eq!(shown, *game, "{what}");
        assert!(rest.is_empty(), "{what}: nothing after the tag");

        let (read, rest) = read_value(tag, &payload).expect("the tag reads");
        assert!(rest.is_empty());
        assert_eq!(
            component_from(kind, &read).as_ref(),
            Ok(&component),
            "{what} reads back"
        );
    }
}

#[test]
fn a_list_of_ints_is_written_as_a_list_tag_and_an_int_array_reads_as_one() {
    let explosion = Component::Fireworks(Fireworks {
        flight_duration: 1,
        explosions: vec![FireworkExplosion {
            shape: FireworkShape::Burst,
            colours: vec![1, 2, 3],
            fade_colours: Vec::new(),
            trail: false,
            twinkle: true,
        }],
    });
    let (tag, payload) = written(&structure(&explosion));
    let (shown, _) = snbt(tag, &payload);
    assert_eq!(
        shown,
        r#"{explosions:[{colors:[1,2,3],has_twinkle:1b,shape:"burst"}],flight_duration:1b}"#
    );

    // The same explosion with its colours as an `IntArrayTag`, as an older game or a
    // tool might have written them.
    let mut colours = Vec::new();
    colours.extend_from_slice(&3i32.to_be_bytes());
    for colour in [1i32, 2, 3] {
        colours.extend_from_slice(&colour.to_be_bytes());
    }
    let (Value::List(items), _) = read_value(TAG_INT_ARRAY, &colours).expect("it reads") else {
        panic!("an int array reads as a list");
    };
    assert_eq!(items, vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
}

#[test]
fn a_list_of_mixed_types_wraps_its_elements_and_reads_back_unwrapped() {
    let mixed = Value::List(vec![
        Value::Int(1),
        Value::Map(vec![("a".to_owned(), Value::Byte(2))]),
        Value::Map(vec![(String::new(), Value::Byte(3))]),
    ]);
    let (tag, payload) = written(&mixed);
    let (shown, _) = snbt(tag, &payload);
    assert_eq!(shown, r#"[{"":1},{a:2b},{"":{"":3b}}]"#);
    let (read, _) = read_value(tag, &payload).expect("it reads");
    assert_eq!(read, mixed);
}

#[test]
fn a_compound_of_more_than_twelve_entries_goes_in_the_order_of_a_grown_hash_map() {
    // A one-letter name's `hashCode` is its character, which `HashMap.hash` leaves as
    // it is: q is 113, a to l are 97 to 108. In a table of 16, q and a share bucket 1
    // (113 & 15 = 97 & 15 = 1), q first as it was put first, and b to l are buckets 2
    // to 12. The thirteenth entry makes the map hold more than 12, three quarters of
    // 16, and it doubles to 32: a to l stay in buckets 1 to 12, and q goes to 17
    // (113 & 31), after l.
    let names = [
        "q", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l",
    ];
    let order_of = |names: &[&str]| -> Vec<String> {
        let value = Value::Map(
            names
                .iter()
                .map(|name| ((*name).to_owned(), Value::Byte(0)))
                .collect(),
        );
        let (_, payload) = written(&value);
        let Some((Value::Map(entries), _)) = read_value(TAG_COMPOUND, &payload) else {
            panic!("a compound reads");
        };
        entries.into_iter().map(|(name, _)| name).collect()
    };
    assert_eq!(
        order_of(&names),
        [
            "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "q"
        ]
    );
    // Twelve entries are within the table of 16: q before a, by their putting in.
    assert_eq!(
        order_of(&names[..12]),
        ["q", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"]
    );
}

fn potion(potion: &str) -> Component {
    Component::PotionContents(PotionContents {
        potion: Some(potion.to_owned()),
        ..PotionContents::default()
    })
}

fn with(item: i32, count: i32, components: &[Component]) -> ItemStack {
    let mut stack = ItemStack::new(item, count);
    for component in components {
        stack.components.set(component.clone());
    }
    stack
}

#[test]
fn a_chest_with_a_potion_a_tipped_arrow_and_stone_reads_back_as_it_was_written() {
    let mut chest = read_container(27, &[]);
    chest.slots[0] = Some(with(items::POTION, 1, &[potion("minecraft:swiftness")]));
    chest.slots[5] = Some(with(
        items::TIPPED_ARROW,
        16,
        &[Component::PotionContents(PotionContents {
            potion: Some("minecraft:poison".to_owned()),
            custom_colour: Some(0x00ff00),
            custom_effects: vec![EffectInstance {
                effect: "minecraft:speed".to_owned(),
                amplifier: 1,
                duration: 200,
                ambient: false,
                visible: true,
                show_icon: true,
            }],
            custom_name: None,
        })],
    ));
    chest.slots[26] = Some(ItemStack::new(items::STONE, 64));
    let data = container_data(blocks::CHEST, &chest, &[]);
    assert_eq!(read_container(27, &data), chest);
}

#[test]
fn a_plain_stacks_bytes_are_slot_id_and_count_in_the_order_of_their_buckets() {
    // The buckets in a table of 16: count 8, Slot 9, id 11 (and components 2, not
    // written for a plain stack).
    let mut chest = read_container(27, &[]);
    chest.slots[3] = Some(ItemStack::new(items::STONE, 5));
    chest.slots[4] = Some(ItemStack::new(items::STONE, 1));
    let data = container_data(blocks::CHEST, &chest, &[]);
    let mut items = vec![TAG_COMPOUND];
    items.extend_from_slice(&2i32.to_be_bytes());
    for (slot, count) in [(3u8, 5i32), (4, 1)] {
        items.push(TAG_INT);
        nbt_put_string(&mut items, "count");
        items.extend_from_slice(&count.to_be_bytes());
        items.push(TAG_BYTE);
        nbt_put_string(&mut items, "Slot");
        items.push(slot);
        items.push(TAG_STRING);
        nbt_put_string(&mut items, "id");
        nbt_put_string(&mut items, "minecraft:stone");
        items.push(TAG_END);
    }
    let mut expected = vec![TAG_COMPOUND];
    // components (2), then Items (10).
    expected.push(TAG_COMPOUND);
    nbt_put_string(&mut expected, "components");
    expected.push(TAG_END);
    expected.push(TAG_LIST);
    nbt_put_string(&mut expected, "Items");
    expected.extend_from_slice(&items);
    expected.push(TAG_END);
    assert_eq!(data, expected);
}

#[test]
fn a_removed_kind_is_written_after_a_bang_and_read_back() {
    let mut stack = ItemStack::new(items::CROSSBOW, 1);
    stack.components.remove(Kind::ChargedProjectiles);
    stack.components.set(Component::Damage(3));
    let value = slot_stack_value(2, &stack);
    let (tag, payload) = written(&value);
    let (shown, _) = snbt(tag, &payload);
    assert_eq!(
        shown,
        r#"{Slot:2b,components:{"!minecraft:charged_projectiles":{},"minecraft:damage":3},count:1,id:"minecraft:crossbow"}"#
    );
    let Some((Value::Map(fields), _)) = read_value(tag, &payload) else {
        panic!("a compound reads");
    };
    assert_eq!(slot_of(&fields), Some(2));
    assert_eq!(stack_from_fields(&fields), Some(stack));
}

/// A chest's data as the game would write one: a potion in slot 1 named in an anvil,
/// with a lore line, and a kind of the set whose value does not read.
fn game_written_chest() -> Vec<u8> {
    let mut stack = vec![TAG_BYTE];
    nbt_put_string(&mut stack, "Slot");
    stack.push(1);
    stack.push(TAG_STRING);
    nbt_put_string(&mut stack, "id");
    nbt_put_string(&mut stack, "minecraft:potion");
    stack.push(TAG_INT);
    nbt_put_string(&mut stack, "count");
    stack.extend_from_slice(&1i32.to_be_bytes());
    stack.push(TAG_COMPOUND);
    nbt_put_string(&mut stack, "components");
    {
        stack.push(TAG_STRING);
        nbt_put_string(&mut stack, "minecraft:custom_name");
        nbt_put_string(&mut stack, "Quick");
        stack.push(TAG_COMPOUND);
        nbt_put_string(&mut stack, "minecraft:potion_contents");
        stack.push(TAG_STRING);
        nbt_put_string(&mut stack, "potion");
        nbt_put_string(&mut stack, "minecraft:swiftness");
        stack.push(TAG_END);
        stack.push(TAG_LIST);
        nbt_put_string(&mut stack, "minecraft:lore");
        stack.push(TAG_STRING);
        stack.extend_from_slice(&1i32.to_be_bytes());
        nbt_put_string(&mut stack, "fast");
        // A damage that is a string: the game's codec refuses it.
        stack.push(TAG_STRING);
        nbt_put_string(&mut stack, "minecraft:damage");
        nbt_put_string(&mut stack, "a lot");
        stack.push(TAG_END);
    }
    stack.push(TAG_END);

    let mut data = vec![TAG_COMPOUND, TAG_LIST];
    nbt_put_string(&mut data, "Items");
    data.push(TAG_COMPOUND);
    data.extend_from_slice(&1i32.to_be_bytes());
    data.extend_from_slice(&stack);
    data.push(TAG_COMPOUND);
    nbt_put_string(&mut data, "components");
    data.push(TAG_END);
    data.push(TAG_END);
    data
}

#[test]
fn a_game_written_stack_keeps_what_clustine_knows_and_drops_the_rest() {
    let chest = read_container(27, &game_written_chest());
    assert_eq!(
        chest.slots[1],
        Some(with(items::POTION, 1, &[potion("minecraft:swiftness")])),
        "the custom name, the lore and the damage that does not read are dropped"
    );
    assert_eq!(chest.slots.iter().flatten().count(), 1);
}

#[test]
fn a_stack_whose_components_all_drop_is_a_plain_stack() {
    let mut fields = vec![
        ("id".to_owned(), Value::String("minecraft:stone".to_owned())),
        ("count".to_owned(), Value::Byte(3)),
    ];
    fields.push((
        "components".to_owned(),
        Value::Map(vec![(
            "minecraft:custom_name".to_owned(),
            Value::String("x".to_owned()),
        )]),
    ));
    let stack = stack_from_fields(&fields).expect("the stack reads");
    assert_eq!(stack, ItemStack::new(items::STONE, 3));
    assert_eq!(stack.components, Components::NONE);
}

#[test]
fn a_stack_without_a_count_is_one_and_a_kind_without_a_namespace_is_the_games() {
    let fields = vec![
        (
            "id".to_owned(),
            Value::String("minecraft:potion".to_owned()),
        ),
        (
            "components".to_owned(),
            Value::Map(vec![(
                "potion_contents".to_owned(),
                Value::Map(vec![(
                    "potion".to_owned(),
                    Value::String("minecraft:swiftness".to_owned()),
                )]),
            )]),
        ),
    ];
    assert_eq!(
        stack_from_fields(&fields),
        Some(with(items::POTION, 1, &[potion("minecraft:swiftness")]))
    );
}
