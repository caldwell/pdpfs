// Copyright © 2023 David Caldwell <david@porkrind.org>

use neon::prelude::*;

use std::{fmt::{Debug, Display}, path::{PathBuf, Path}, convert::TryFrom};

#[derive(Debug)]
pub enum Error {
    Throw(neon::result::Throw),
    Std(Box<dyn std::error::Error>),
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::convert::From<neon::result::Throw> for Error {
    fn from(value: neon::result::Throw) -> Self {
        Error::Throw(value)
    }
}

impl std::convert::From<String> for Error {
    fn from(value: String) -> Self {
        let e: Box<dyn std::error::Error> = value.into();
        Error::Std(e)
    }
}

pub trait FromJs : Sized {
    type Input: Value;
    fn from(cx: &mut FunctionContext, from: Handle<Self::Input>) -> NeonResult<Self>;
}
pub trait ToJs<'a,'b> : Sized {
    type Output: Value;
    fn to(&'b self, cx: &mut FunctionContext<'a>) -> NeonResult<Handle<'a,Self::Output>>;
}

impl FromJs for bool {
    type Input=JsBoolean;
    fn from(cx: &mut FunctionContext, from: Handle<Self::Input>) -> NeonResult<Self> {
        Ok(from.value(cx))
    }
}

impl<'a,'b> ToJs<'a,'b> for bool {
    type Output=JsBoolean;
    fn to(&'b self, cx: &mut FunctionContext<'a>) -> NeonResult<Handle<'a,Self::Output>> {
        Ok(cx.boolean(*self))
    }
}

impl FromJs for u32 {
    type Input=JsNumber;
    fn from(cx: &mut FunctionContext, from: Handle<Self::Input>) -> NeonResult<Self> {
        Ok(from.value(cx) as u32)
    }
}

impl<'a,'b> ToJs<'a,'b> for u32 {
    type Output=JsNumber;
    fn to(&'b self, cx: &mut FunctionContext<'a>) -> NeonResult<Handle<'a,Self::Output>> {
        Ok(cx.number(*self))
    }
}

impl FromJs for String {
    type Input=JsString;
    fn from(cx: &mut FunctionContext, from: Handle<Self::Input>) -> NeonResult<Self> {
        Ok(from.value(cx))
    }
}

impl<'a,'b> ToJs<'a,'b> for String {
    type Output=JsString;
    fn to(&'b self, cx: &mut FunctionContext<'a>) -> NeonResult<Handle<'a,Self::Output>> {
        Ok(cx.string(self))
    }
}

impl<'a,'b> ToJs<'a,'b> for &'a str {
    type Output=JsString;
    fn to(&'b self, cx: &mut FunctionContext<'a>) -> NeonResult<Handle<'a,Self::Output>> {
        Ok(cx.string(self))
    }
}

impl FromJs for PathBuf {
    type Input=JsString;
    fn from(cx: &mut FunctionContext, from: Handle<Self::Input>) -> NeonResult<Self> {
        Ok(Path::new(&from.value(cx)).to_owned())
    }
}

impl<'a,'b> ToJs<'a,'b> for PathBuf {
    type Output=JsString;
    fn to(&'b self, cx: &mut FunctionContext<'a>) -> NeonResult<Handle<'a,Self::Output>> {
        let s = self.to_str().ok_or_else(|| format!("Couldn't convert {} to String", self.display())).into_jserr(cx)?;
        Ok(cx.string(s))
    }
}

impl FromJs for pdpfs::ops::ImageType {
    type Input=JsString;
    fn from(cx: &mut FunctionContext, from: Handle<Self::Input>) -> NeonResult<Self> {
        Self::try_from(from.value(cx).as_str())
            .map_err(|_| format!("Unknown image type: {}", from.value(cx))).into_jserr(cx)
    }
}

impl FromJs for pdpfs::ops::DeviceType {
    type Input=JsString;
    fn from(cx: &mut FunctionContext, from: Handle<Self::Input>) -> NeonResult<Self> {
        Self::try_from(from.value(cx).as_str())
            .map_err(|_| format!("Unknown device type: {}", from.value(cx))).into_jserr(cx)
    }
}

impl FromJs for pdpfs::ops::FileSystemType {
    type Input=JsString;
    fn from(cx: &mut FunctionContext, from: Handle<Self::Input>) -> NeonResult<Self> {
        Self::try_from(from.value(cx).as_str())
            .map_err(|_| format!("Unknown filesystem type: {}", from.value(cx))).into_jserr(cx)
    }
}

impl<T: FromJs> FromJs for Option<T> {
    type Input=JsValue;
    fn from(cx: &mut FunctionContext, from: Handle<Self::Input>) -> NeonResult<Self> {
        if from.is_a::<JsNull, _>(cx) || from.is_a::<JsUndefined, _>(cx) {
            Ok(None)
        } else {
            let v = from.downcast(cx).map_err(|e| format!("{:?}", e)).into_jserr(cx)?;
            Ok(Some(T::from(cx, v)?))
        }
    }
}

impl<'a,'b,T: ToJs<'a,'b>> ToJs<'a,'b> for Option<T> {
    type Output=JsValue;
    fn to(&'b self, cx: &mut FunctionContext<'a>) -> NeonResult<Handle<'a,Self::Output>> {
        match self {
            None => Ok(cx.null().as_value(cx)),
            Some(v) => v.to(cx).map(|v| v.as_value(cx)),
        }
    }
}

impl<'a,'b> ToJs<'a,'b> for Handle<'a,JsValue> {
    type Output=JsValue;
    fn to(&'b self, cx: &mut FunctionContext<'a>) -> NeonResult<Handle<'a,Self::Output>> {
        Ok(self.as_value(cx))
    }
}

pub fn argument<T: FromJs>(cx: &mut FunctionContext, num: i32) -> NeonResult<T> {
    let val = cx.argument::<T::Input>(num)?;
    T::from(cx, val)
}

#[macro_export]
macro_rules! js_args {
    ($cx:expr, $( $arg_name:ident:$type:ty ),*) => {
        let cx: &mut FunctionContext = $cx;
        let mut i = 0;
        $(
          let $arg_name = crate::make_neon_usable::argument::<$type>(cx, i)?;
          #[allow(unused_assignments)] {
              i += 1;
          }
        )*
    };
}

#[allow(unused_macros)]
macro_rules! js_args2 {
    ($cx:expr, $( $rest:tt )*) => { js_args!(@decl $cx; ; $($rest)*) };

    (@decl $cx:expr; $( $name:ident: $fake_type:tt ),*; ) => {
        let cx: &mut FunctionContext = $cx;
        let mut i = 0;
        $(
          let $name = cx.argument::<js_args!(@js_type_for $fake_type)>(i)?.value(cx);
          js_args!(@prep: cx, $fake_type, $name);
          let $name = js_args!(@convert $fake_type, $name);
          #[allow(unused_assignments)] {
              i += 1;
          }
        )*
    };

    (@decl $cx:expr; $( $arg_name:ident: $fake_type:tt ),*; $name:ident : &mut impl FileSystem $(, $( $rest:tt )+ )?) => { js_args!(@decl $cx; $($arg_name: $fake_type,)* $name:mut_impl_filesystem; $( $( $rest )* )*) };
    (@decl $cx:expr; $( $arg_name:ident: $fake_type:tt ),*; $name:ident : $type:tt             $(, $( $rest:tt )+ )?) => { js_args!(@decl $cx; $($arg_name: $fake_type,)* $name:$type;               $( $( $rest )* )*) };

    (@js_type_for u32) => { JsNumber };
    (@js_type_for bool) => { JsBoolean };
    (@js_type_for String) => { JsString };
    (@js_type_for PathBuf) => { JsString };
    (@js_type_for mut_impl_filesystem) => { JsNumber };
    (@js_type_for_tt $( $type:tt )*) => { js_args!(@jstype_for $( $type )*) };

    (@prep: $cx:ident, mut_impl_filesystem, $name:ident) => {
        let id = $name as u32;
        let mut images = IMAGES.lock().unwrap();
        let Some($name) = images.get_mut(&id) else {
            return $cx.throw_error("Bad ID");
        };
    };
    (@prep: $cx:ident, $t:ty, $v:ident) => { };

    (@convert u32, $v:expr) => { $v as u32 };
    (@convert PathBuf, $v:expr) => { Path::new(&$v).to_owned() };
    (@convert mut_impl_filesystem, $v:expr) => { $v };
    (@convert $t:ty, $v:expr) => { $v }; // for boring stuff
}

pub fn obj_set_string<'a, C: Context<'a>>(cx: &mut C, obj: &Handle<JsObject>, k: &str, v: &str) -> NeonResult<()> {
    let s = cx.string(v);
    obj.set(cx, k, s)?;
    Ok(())
}
pub fn obj_set_bool<'a, C: Context<'a>>(cx: &mut C, obj: &Handle<JsObject>, k: &str, v: bool) -> NeonResult<()> {
    let b = cx.boolean(v);
    obj.set(cx, k, b)?;
    Ok(())
}
pub fn obj_set_number<'a, C: Context<'a>, N>(cx: &mut C, obj: &Handle<JsObject>, k: &str, v: N) -> NeonResult<()>
where N: Into<f64> {
    let s = cx.number(v);
    obj.set(cx, k, s)?;
    Ok(())
}
pub fn obj_set_null<'a, C: Context<'a>>(cx: &mut C, obj: &Handle<JsObject>, k: &str) -> NeonResult<()> {
    let n = cx.null();
    obj.set(cx, k, n)?;
    Ok(())
}

// Seriously, neon????
pub fn vec_to_array<'a,'b, T:ToJs<'a,'b>>(cx: &mut FunctionContext<'a>, vec: &'b [T]) -> JsResult<'a, JsArray> {
    let a = JsArray::new(cx, vec.len() as u32);
    for (i, v) in vec.iter().enumerate() {
        let jv = v.to(cx)?;
        a.set(cx, i as u32, jv)?;
    }
    Ok(a)
}

pub trait ToJsResult<T, E: Debug+Display> {
    fn into_jserr(self, cx: &mut FunctionContext) -> NeonResult<T>;
}

impl<T,E: Debug+Display+ToThrow> ToJsResult<T,E> for Result<T,E> {
    fn into_jserr(self, cx: &mut FunctionContext) -> NeonResult<T> {
        self.map_err(|e| e.into_throw(cx))
    }
}

trait ToThrow {
    fn into_throw(self, cx: &mut FunctionContext) -> neon::result::Throw;
}

impl ToThrow for Error {
    fn into_throw(self, cx: &mut FunctionContext) -> neon::result::Throw {
        match self {
            Error::Throw(t) => t,
            Error::Std(e) => cx.throw_error::<_,()>(e.to_string()).unwrap_err(),
        }
    }
}

impl ToThrow for String {
    fn into_throw(self, cx: &mut FunctionContext) -> neon::result::Throw {
        cx.throw_error::<_,()>(self).unwrap_err()
    }
}
