//! Components by name, both ways: a `&dyn PartialReflect` read as a Ruby value, and a Ruby
//! value written back over one.
//!
//! There is no glue per component type here. Bevy's reflection says what a `Transform` is made
//! of at run time — a struct of three fields, one of them a `glam::Vec3` of three `f32` — and
//! that is enough to build the Hash a script sees and to read one back. A game's own component
//! joins in by being `#[derive(Reflect)] #[reflect(Component)]` and registered
//! (`app.register_type::<T>()`); nothing in rubevy names it.
//!
//! **What a value looks like on the Ruby side.**
//!
//! | reflected | Ruby |
//! |---|---|
//! | struct | Hash, keys as Symbols (`{translation: …, rotation: …}`) |
//! | `glam::Vec2` / `Vec3` / `Vec3A` / `Vec4` / `Quat` | Array of Floats — these are structs of `x`, `y`, … in bevy_reflect, but a script wants `tf[:translation][0]` |
//! | tuple struct, tuple, array, list | Array |
//! | map | Hash |
//! | set | Array |
//! | enum, unit variant | the variant's name as a Symbol (`:Hidden`) |
//! | enum, tuple or struct variant | a one-entry Hash, `{Srgba: {red: …}}` |
//! | opaque | Float, Integer, true/false, String, or a `Rubevy::Entity` — see [`opaque_to_ruby`] |
//! | anything else | nil |
//!
//! **What a write does.** A Hash is applied field by field, so a field the Hash does not name
//! is left as it was — which is what makes reading a component, changing one number and
//! writing it back cost one round trip rather than two. An Array applied to a struct goes by
//! position, which is how `[1.0, 2.0, 3.0]` reaches a `Vec3`. A Symbol switches an enum to
//! that (unit) variant. A value of a kind the field cannot take is reported, not applied.

use bevy::prelude::*;
use bevy::reflect::enums::{DynamicEnum, DynamicVariant, VariantType};
use bevy::reflect::{PartialReflect, ReflectMut, ReflectRef};

use sabiruby::value::ObjId;
use sabiruby::{Value, Vm, VmError};

/// How deep the conversion follows a value in either direction. A component nested past this
/// is a script's problem, not the host's: the point of the boundary is a Hash a script can
/// read, and the limit keeps a cycle (a `Map` holding itself, a Ruby Hash holding itself) from
/// taking the stack with it.
const MAX_DEPTH: usize = 16;

/// The math types a script is given as an Array rather than as a Hash of `x`, `y`, `z`.
///
/// In bevy_reflect 0.19 these are structs, not opaque values (`bevy_reflect/src/impls/glam.rs`
/// spells them out with `impl_reflect!`), so the fields *are* reachable by name. A script that
/// says `tf[:translation][0] += 1.0` wants the Array all the same, and a position that is three
/// numbers reads better than a Hash of one-letter keys. The other way round needs no list: an
/// Array written over any struct goes by position (see [`apply_ruby`]).
const AS_ARRAY: [&str; 5] = ["glam::Vec2", "glam::Vec3", "glam::Vec3A", "glam::Vec4", "glam::Quat"];

/// A Ruby value the host has taken a copy of, out of the VM and owned by Rust.
///
/// A native is the only place a Ruby Hash can be read — it is the only place with a `&mut Vm` —
/// but the component it is meant for is written a system later, when there is a `&mut World`
/// and no VM. So the native reads the whole value out here and the command carries it. The
/// other way (holding the `Value` and registering it with the collector until the write
/// happens) would keep a Ruby object alive across a frame boundary for no gain: the script has
/// already moved on, and a Hash it changes in the meantime would change the write.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RubyData {
    Nil,
    Bool(bool),
    Int(i64),
    Num(f64),
    Text(String),
    /// A Symbol, by its name: what an enum's variant is written as.
    Sym(String),
    List(Vec<RubyData>),
    /// A Hash, in the order its keys came out of it.
    Map(Vec<(RubyData, RubyData)>),
    /// A `Rubevy::Entity` object, for a field that holds an `Entity`.
    Entity(Entity),
}

impl RubyData {
    /// The name a key stands for: a Symbol or a String, which is how both a field and a
    /// variant may be written.
    fn as_name(&self) -> Option<&str> {
        match self {
            RubyData::Sym(s) | RubyData::Text(s) => Some(s),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            RubyData::Int(i) => Some(*i as f64),
            RubyData::Num(n) => Some(*n),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------ reading a Ruby value

/// Reads a Ruby value out of the VM, as far as [`MAX_DEPTH`].
///
/// `entity_tag` is what a `Rubevy::Entity` object carries in its `Vm::data_of` tag, so that an
/// entity a script holds stays an entity instead of becoming a number.
pub(crate) fn read_ruby(vm: &mut Vm, v: Value, entity_tag: u32) -> Result<RubyData, VmError> {
    read_at(vm, v, entity_tag, 0)
}

fn read_at(vm: &mut Vm, v: Value, entity_tag: u32, depth: usize) -> Result<RubyData, VmError> {
    if depth > MAX_DEPTH {
        return Ok(RubyData::Nil);
    }
    if let Some((tag, handle)) = vm.data_of(v)
        && tag == entity_tag
    {
        return Ok(match Entity::try_from_bits(handle) {
            Some(e) => RubyData::Entity(e),
            None => RubyData::Nil,
        });
    }
    Ok(match v {
        Value::Nil => RubyData::Nil,
        Value::True => RubyData::Bool(true),
        Value::False => RubyData::Bool(false),
        Value::Int(i) => RubyData::Int(i),
        Value::Float(f) => RubyData::Num(f),
        Value::Sym(s) => RubyData::Sym(vm.sym_name(s)),
        other => {
            if let Some(items) = vm.ary_vals(other) {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(read_at(vm, it, entity_tag, depth + 1)?);
                }
                return Ok(RubyData::List(out));
            }
            if vm.obj_is_kind_of(other, vm.core.hash) {
                // The VM has `hash_new`, `hash_set` and `hash_get` but no way to walk a Hash
                // from Rust, so the keys come back through Ruby's own `keys` — which is what a
                // native may do (it returns at once; nothing here waits).
                let keys = vm.intern("keys");
                let keys = vm.funcall(other, keys, &[], Value::Nil)?;
                let keys = vm.ary_vals(keys).unwrap_or_default();
                let mut out = Vec::with_capacity(keys.len());
                for k in keys {
                    let value = vm.hash_get(other, k).unwrap_or(Value::Nil);
                    let k = read_at(vm, k, entity_tag, depth + 1)?;
                    let value = read_at(vm, value, entity_tag, depth + 1)?;
                    out.push((k, value));
                }
                return Ok(RubyData::Map(out));
            }
            match vm.str_bytes(other) {
                Some(b) => RubyData::Text(String::from_utf8_lossy(b).into_owned()),
                None => RubyData::Nil,
            }
        }
    })
}

// ------------------------------------------------------------------ reflection → Ruby

/// The Ruby value for a reflected one. `entity_class` is `Rubevy::Entity`, for a field that
/// holds an `Entity`.
pub(crate) fn reflect_to_ruby(
    vm: &mut Vm,
    value: &dyn PartialReflect,
    entity_class: ObjId,
    entity_tag: u32,
) -> Value {
    to_ruby(vm, value, entity_class, entity_tag, 0)
}

fn to_ruby(
    vm: &mut Vm,
    value: &dyn PartialReflect,
    entity_class: ObjId,
    entity_tag: u32,
    depth: usize,
) -> Value {
    if depth > MAX_DEPTH {
        return Value::Nil;
    }
    let deeper = depth + 1;
    match value.reflect_ref() {
        ReflectRef::Struct(s) => {
            if AS_ARRAY.contains(&value.reflect_type_path()) {
                let items: Vec<Value> = (0..s.field_len())
                    .map(|i| match s.field_at(i) {
                        Some(f) => to_ruby(vm, f, entity_class, entity_tag, deeper),
                        None => Value::Nil,
                    })
                    .collect();
                return vm.ary_new(items);
            }
            let h = vm.hash_new();
            for i in 0..s.field_len() {
                let (Some(name), Some(f)) = (s.name_at(i), s.field_at(i)) else { continue };
                let k = Value::Sym(vm.intern(name));
                let v = to_ruby(vm, f, entity_class, entity_tag, deeper);
                let _ = vm.hash_set(h, k, v);
            }
            h
        }
        ReflectRef::TupleStruct(t) => {
            let items: Vec<Value> =
                t.iter_fields().map(|f| to_ruby(vm, f, entity_class, entity_tag, deeper)).collect();
            vm.ary_new(items)
        }
        ReflectRef::Tuple(t) => {
            let items: Vec<Value> =
                t.iter_fields().map(|f| to_ruby(vm, f, entity_class, entity_tag, deeper)).collect();
            vm.ary_new(items)
        }
        ReflectRef::List(l) => {
            let items: Vec<Value> =
                l.iter().map(|f| to_ruby(vm, f, entity_class, entity_tag, deeper)).collect();
            vm.ary_new(items)
        }
        ReflectRef::Array(a) => {
            let items: Vec<Value> =
                a.iter().map(|f| to_ruby(vm, f, entity_class, entity_tag, deeper)).collect();
            vm.ary_new(items)
        }
        ReflectRef::Set(s) => {
            let items: Vec<Value> =
                s.iter().map(|f| to_ruby(vm, f, entity_class, entity_tag, deeper)).collect();
            vm.ary_new(items)
        }
        ReflectRef::Map(m) => {
            let h = vm.hash_new();
            for (k, v) in m.iter() {
                let k = to_ruby(vm, k, entity_class, entity_tag, deeper);
                let v = to_ruby(vm, v, entity_class, entity_tag, deeper);
                let _ = vm.hash_set(h, k, v);
            }
            h
        }
        ReflectRef::Enum(e) => {
            let name = Value::Sym(vm.intern(e.variant_name()));
            match e.variant_type() {
                // `Visibility::Hidden` is `:Hidden`, which is what a script writes back
                VariantType::Unit => name,
                VariantType::Tuple => {
                    let items: Vec<Value> = e
                        .iter_fields()
                        .map(|f| to_ruby(vm, f.value(), entity_class, entity_tag, deeper))
                        .collect();
                    let items = vm.ary_new(items);
                    let h = vm.hash_new();
                    let _ = vm.hash_set(h, name, items);
                    h
                }
                VariantType::Struct => {
                    let fields = vm.hash_new();
                    for f in e.iter_fields() {
                        let Some(n) = f.name() else { continue };
                        let k = Value::Sym(vm.intern(n));
                        let v = to_ruby(vm, f.value(), entity_class, entity_tag, deeper);
                        let _ = vm.hash_set(fields, k, v);
                    }
                    let h = vm.hash_new();
                    let _ = vm.hash_set(h, name, fields);
                    h
                }
            }
        }
        ReflectRef::Opaque(o) => opaque_to_ruby(vm, o, entity_class, entity_tag),
        // `functions` is not a feature this crate turns on; the arm is here so that a bevy
        // built with it still compiles.
        #[allow(unreachable_patterns)]
        _ => Value::Nil,
    }
}

/// The opaque types a script is given a value for: the numbers, `bool`, a String, and
/// `Entity` (as the `Rubevy::Entity` object, not as a number). Anything else — a handle, a
/// type of the game's own that is `#[reflect(opaque)]` — is nil, because a Ruby value for it
/// would be a guess.
fn opaque_to_ruby(
    vm: &mut Vm,
    value: &dyn PartialReflect,
    entity_class: ObjId,
    entity_tag: u32,
) -> Value {
    macro_rules! float {
        ($($t:ty),*) => { $(if let Some(x) = value.try_downcast_ref::<$t>() { return Value::Float(*x as f64) })* };
    }
    macro_rules! int {
        ($($t:ty),*) => { $(if let Some(x) = value.try_downcast_ref::<$t>() { return Value::Int(*x as i64) })* };
    }
    float!(f32, f64);
    int!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
    if let Some(b) = value.try_downcast_ref::<bool>() {
        return Value::bool(*b);
    }
    if let Some(s) = value.try_downcast_ref::<String>() {
        return vm.str_new(s.as_bytes());
    }
    if let Some(c) = value.try_downcast_ref::<char>() {
        let mut buf = [0u8; 4];
        return vm.str_new(c.encode_utf8(&mut buf).as_bytes());
    }
    if let Some(e) = value.try_downcast_ref::<Entity>() {
        return vm.data_new(entity_class, entity_tag, e.to_bits());
    }
    Value::Nil
}

// ------------------------------------------------------------------ Ruby → reflection

/// Writes a Ruby value over a reflected one, in place.
///
/// Partial on purpose: a Hash names the fields it means and the others keep their values, so a
/// script that reads a `Transform`, moves it by one and writes it back does not have to send
/// every field — and two scripts that touch different fields of the same component do not
/// undo each other.
///
/// The `Err` is a sentence for the log, naming the path inside the component (`translation.x`)
/// rather than only the component. A mismatch stops that field, not the whole write.
pub(crate) fn apply_ruby(dest: &mut dyn PartialReflect, value: &RubyData) -> Result<(), String> {
    let mut problems = Vec::new();
    apply_at(dest, value, "", 0, &mut problems);
    if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
}

fn apply_at(
    dest: &mut dyn PartialReflect,
    value: &RubyData,
    path: &str,
    depth: usize,
    problems: &mut Vec<String>,
) {
    if depth > MAX_DEPTH {
        problems.push(format!("{path}: too deep"));
        return;
    }
    let deeper = depth + 1;
    // An enum is settled before the value is looked at, because a Symbol names a variant and
    // switching one is not a field write.
    if let ReflectMut::Enum(_) = dest.reflect_mut() {
        apply_enum(dest, value, path, deeper, problems);
        return;
    }
    match dest.reflect_mut() {
        ReflectMut::Struct(s) => match value {
            RubyData::Map(entries) => {
                for (k, v) in entries {
                    let Some(name) = k.as_name() else {
                        problems.push(format!("{path}: a field name must be a Symbol or a String"));
                        continue;
                    };
                    let here = join(path, name);
                    match s.field_mut(name) {
                        Some(f) => apply_at(f, v, &here, deeper, problems),
                        None => problems.push(format!("{here}: no such field")),
                    }
                }
            }
            // `[1.0, 2.0, 3.0]` over a `Vec3`, and over any other struct by position
            RubyData::List(items) => {
                for (i, v) in items.iter().enumerate() {
                    let here = join(path, &i.to_string());
                    match s.field_at_mut(i) {
                        Some(f) => apply_at(f, v, &here, deeper, problems),
                        None => problems.push(format!("{here}: past the end of the struct")),
                    }
                }
            }
            _ => problems.push(format!("{path}: a struct takes a Hash or an Array")),
        },
        ReflectMut::TupleStruct(t) => match value {
            RubyData::List(items) => {
                for (i, v) in items.iter().enumerate() {
                    let here = join(path, &i.to_string());
                    match t.field_mut(i) {
                        Some(f) => apply_at(f, v, &here, deeper, problems),
                        None => problems.push(format!("{here}: past the end")),
                    }
                }
            }
            // a newtype (`struct Hp(f32)`) takes the number on its own as well
            other => match t.field_mut(0) {
                Some(f) => apply_at(f, other, &join(path, "0"), deeper, problems),
                None => problems.push(format!("{path}: takes an Array")),
            },
        },
        ReflectMut::Tuple(t) => match value {
            RubyData::List(items) => {
                for (i, v) in items.iter().enumerate() {
                    let here = join(path, &i.to_string());
                    match t.field_mut(i) {
                        Some(f) => apply_at(f, v, &here, deeper, problems),
                        None => problems.push(format!("{here}: past the end")),
                    }
                }
            }
            _ => problems.push(format!("{path}: a tuple takes an Array")),
        },
        ReflectMut::Array(a) => match value {
            RubyData::List(items) => {
                for (i, v) in items.iter().enumerate() {
                    let here = join(path, &i.to_string());
                    match a.get_mut(i) {
                        Some(f) => apply_at(f, v, &here, deeper, problems),
                        None => problems.push(format!("{here}: past the end of the array")),
                    }
                }
            }
            _ => problems.push(format!("{path}: an array takes an Array")),
        },
        // A List could be grown from Ruby, but that needs a value of the element's type to
        // push and reflection alone does not make one. What is here is what can be done
        // safely: the items that exist are written over.
        ReflectMut::List(l) => match value {
            RubyData::List(items) => {
                for (i, v) in items.iter().enumerate() {
                    let here = join(path, &i.to_string());
                    match l.get_mut(i) {
                        Some(f) => apply_at(f, v, &here, deeper, problems),
                        None => problems.push(format!("{here}: a list is not grown from Ruby")),
                    }
                }
            }
            _ => problems.push(format!("{path}: a list takes an Array")),
        },
        ReflectMut::Opaque(o) => apply_opaque(o, value, path, problems),
        _ => problems.push(format!("{path}: this kind of value is not written from Ruby")),
    }
}

/// A Symbol switches the variant; a one-entry Hash names a variant and its fields, and the
/// fields are written in place where it is the variant the value already has.
///
/// Switching to a *tuple or struct* variant is not done: bevy builds the new variant from the
/// value handed to `try_apply`, which means every field of it has to be there and to be of the
/// field's own type — and a Ruby Hash says nothing about types. A unit variant has no fields,
/// so it is exact, and that is what `Visibility`, a state, or a game's own mode is.
fn apply_enum(
    dest: &mut dyn PartialReflect,
    value: &RubyData,
    path: &str,
    depth: usize,
    problems: &mut Vec<String>,
) {
    let current = match dest.reflect_ref() {
        ReflectRef::Enum(e) => e.variant_name().to_string(),
        _ => return,
    };
    match value {
        RubyData::Sym(name) | RubyData::Text(name) => {
            if *name == current {
                return;
            }
            let variant = DynamicEnum::new(name.clone(), DynamicVariant::Unit);
            if let Err(e) = dest.try_apply(&variant) {
                problems.push(format!("{path}: {e}"));
            }
        }
        RubyData::Map(entries) if entries.len() == 1 => {
            let (k, v) = &entries[0];
            let Some(name) = k.as_name() else {
                problems.push(format!("{path}: a variant must be named by a Symbol"));
                return;
            };
            if name != current {
                problems.push(format!(
                    "{path}: the fields of {name} cannot be written while the value is {current}"
                ));
                return;
            }
            let ReflectMut::Enum(e) = dest.reflect_mut() else { return };
            match v {
                RubyData::Map(fields) => {
                    for (k, v) in fields {
                        let Some(field) = k.as_name() else { continue };
                        let here = join(path, field);
                        match e.field_mut(field) {
                            Some(f) => apply_at(f, v, &here, depth, problems),
                            None => problems.push(format!("{here}: no such field")),
                        }
                    }
                }
                RubyData::List(items) => {
                    for (i, v) in items.iter().enumerate() {
                        let here = join(path, &i.to_string());
                        match e.field_at_mut(i) {
                            Some(f) => apply_at(f, v, &here, depth, problems),
                            None => problems.push(format!("{here}: past the end")),
                        }
                    }
                }
                _ => problems.push(format!("{path}: a variant's fields are a Hash or an Array")),
            }
        }
        _ => problems.push(format!("{path}: an enum takes a Symbol or a one-entry Hash")),
    }
}

fn apply_opaque(
    dest: &mut dyn PartialReflect,
    value: &RubyData,
    path: &str,
    problems: &mut Vec<String>,
) {
    macro_rules! number {
        ($($t:ty),*) => { $(if let Some(x) = dest.try_downcast_mut::<$t>() {
            match value.as_f64() {
                Some(n) => { *x = n as $t; return }
                None => { problems.push(format!("{path}: takes a number")); return }
            }
        })* };
    }
    number!(f32, f64, i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
    if let Some(b) = dest.try_downcast_mut::<bool>() {
        // nil and false are false, as they are everywhere in Ruby
        *b = !matches!(value, RubyData::Nil | RubyData::Bool(false));
        return;
    }
    if let Some(s) = dest.try_downcast_mut::<String>() {
        match value {
            RubyData::Text(t) | RubyData::Sym(t) => *s = t.clone(),
            _ => problems.push(format!("{path}: takes a String")),
        }
        return;
    }
    if let Some(e) = dest.try_downcast_mut::<Entity>() {
        match value {
            RubyData::Entity(x) => *e = *x,
            RubyData::Int(bits) => match Entity::try_from_bits(*bits as u64) {
                Some(x) => *e = x,
                None => problems.push(format!("{path}: not an entity")),
            },
            _ => problems.push(format!("{path}: takes a Rubevy::Entity")),
        }
        return;
    }
    problems.push(format!("{path}: this type is not written from Ruby"));
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() { name.to_string() } else { format!("{path}.{name}") }
}
