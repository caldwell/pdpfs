#[macro_use]
extern crate lazy_static;

mod make_neon_usable;

use std::collections::HashMap;
use std::convert::TryFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;
use std::sync::Mutex;

use neon::prelude::*;

use make_neon_usable::*;
use pdpfs::fs::{FileSystem, Timestamp};
use pdpfs::block::BlockDevice;

#[neon::main]
fn main(mut cx: ModuleContext) -> NeonResult<()> {
    cx.export_function("open_image", open_image)?;
    cx.export_function("create_image", create_image)?;
    cx.export_function("close_image", close_image)?;
    cx.export_function("image_is_dirty", image_is_dirty)?;
    cx.export_function("get_directory_entries", get_directory_entries)?;
    cx.export_function("cp_from_image", cp_from_image)?;
    cx.export_function("cp_into_image", cp_into_image)?;
    cx.export_function("mv", mv)?;
    cx.export_function("rm", rm)?;
    cx.export_function("save", save)?;
    cx.export_function("convert", convert)?;
    cx.export_function("filesystem_name", filesystem_name)?;
    cx.export_function("device_types", device_types)?;
    cx.export_function("image_types", image_types)?;
    cx.export_function("filesystems", filesystems)?;
    Ok(())
}

struct Image {
    fs: Box<dyn FileSystem<BlockDevice=Box<dyn BlockDevice>>>,
    dirty: bool,
}

lazy_static! {
    static ref IMAGES: Mutex<HashMap<u32, Image>> = Mutex::new(HashMap::new());
    static ref NEXT_ID: AtomicU32 = AtomicU32::new(0);
}

fn with_image_id<T,E,F>(id: u32, func: F) -> Result<T,Error>
    where F: FnOnce(&mut Image) -> Result<T,E>,
          Error: From<E>,
          E: Into<Error>,
{
    let mut images = IMAGES.lock().unwrap();
    let Some(image) = images.get_mut(&id) else {
        return Err(Error::Std(Box::<dyn std::error::Error>::from(format!("Bad ID"))));
    };
    func(image).map_err(|e| Error::from(e))
}

fn nice_err(err: anyhow::Error) -> String {
    let mut s = format!("{}\n", err);
    err.chain().skip(1).for_each(|cause| s.push_str(&format!("because: {}\n", cause)));
    s
}

fn open_image(mut cx: FunctionContext) -> JsResult<JsNumber> {
    js_args!(&mut cx, image_file: PathBuf);

    let fs = pdpfs::ops::open_fs(pdpfs::ops::open_device(&Path::new(&image_file))
        .map_err(|e| format!("Bad or unknown disk image file format.\nDetails: {}", nice_err(e))).into_jserr(&mut cx)?)
        .map_err(|e| format!("Bad or unknown format on disk image.\nDetails: {}", nice_err(e))).into_jserr(&mut cx)?;

    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    IMAGES.lock().unwrap().insert(id, Image { fs, dirty: false });

    Ok(cx.number(id))
}

fn create_image(mut cx: FunctionContext) -> JsResult<JsNumber> {
    js_args!(&mut cx, image_type: pdpfs::ops::ImageType, device_type: Option<pdpfs::ops::DeviceType>, image_size: Option<u32>, filesystem: pdpfs::ops::FileSystemType);

    let device_type = match (device_type, image_size) {
        (Some(t),   None)    => t,
        (None,      Some(n)) => pdpfs::ops::DeviceType::Flat(n as usize),
        (None,      None)    => return cx.throw_error("create_image: One of device_type or image_size must be specified"),
        (_,         _)       => return cx.throw_error("create_image: Cannot specify both device_type and image_size"),
    };
    let fs = pdpfs::ops::create_image(image_type, device_type, filesystem)
        .map_err(|e| format!("Couldn't create the disk image.\nDetails: {}", e)).into_jserr(&mut cx)?;

    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    IMAGES.lock().unwrap().insert(id, Image { fs, dirty: false });

    Ok(cx.number(id))
}

fn close_image(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32);
    let mut images = IMAGES.lock().unwrap();
    if images.remove(&id).is_none() {
        return Err(format!("Bad ID")).into_jserr(&mut cx);
    };
    Ok(cx.null())
}

fn image_is_dirty(mut cx: FunctionContext) -> JsResult<JsBoolean> {
    js_args!(&mut cx, id: u32);
    let dirty = with_image_id(id, |image| -> Result<bool, Error> {
        Ok(image.dirty)
    }).into_jserr(&mut cx)?;
    Ok(cx.boolean(dirty))
}

fn get_directory_entries(mut cx: FunctionContext) -> JsResult<JsArray> {
    js_args!(&mut cx, id: u32);
    let entries = with_image_id(id, |image| {
        image.fs.read_dir("/")
          .map_err(|err| cx.throw_error::<_,()>(format!("{}", err)).unwrap_err())?
          .map(|e| -> JsResult<JsValue> {
            let obj = cx.empty_object();
            obj_set_bool(&mut cx, &obj, "read_only", e.readonly())?;
            obj_set_string(&mut cx, &obj, "name", &e.file_name())?;
            obj_set_number(&mut cx, &obj, "length", e.len() as u32)?;
            match e.created() {
                Ok(Timestamp::Date(d))      => obj_set_string(&mut cx, &obj, "creation_date", &format!("{}", d))?,
                Ok(Timestamp::DateTime(dt)) => obj_set_string(&mut cx, &obj, "creation_date", &format!("{}", dt))?,
                Err(_) => obj_set_null(&mut cx, &obj, "creation_date")?,
            }
            Ok(obj.upcast())
        }).collect::<NeonResult<Vec<Handle<JsValue>>>>().into()
    }).into_jserr(&mut cx)?;
    vec_to_array(&mut cx, &entries)
}

fn cp_from_image(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, src: String, dest: PathBuf);
    with_image_id(id, |image| {
        pdpfs::ops::cp_from_image(&mut image.fs, &Path::new(&src), &dest)
            .map_err(|e| format!("Could't write {}: {}", dest.display(), e))
            .and_then(|_| { Ok(()) })
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn cp_into_image(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, path: PathBuf);
    with_image_id(id, |image| {
        pdpfs::ops::cp_into_image(&mut image.fs, &path, Path::new("."))
            .map_err(|e| format!("Could't write {}: {}", path.display(), e))
            .and_then(|_| { image.dirty = true; Ok(()) })
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn mv(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, src: PathBuf, dest: PathBuf, overwrite_dest: bool);
    with_image_id(id, |image| {
        pdpfs::ops::mv(&mut image.fs, &src, &dest, overwrite_dest)
            .map_err(|e| format!("mv from {} to {} failed: {}", src.display(), dest.display(), e))
            .and_then(|_| { image.dirty = true; Ok(()) })
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn rm(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, file: PathBuf);
    with_image_id(id, |image| {
        pdpfs::ops::rm(&mut image.fs, &file)
            .map_err(|e| format!("rm failed for {}: {}", file.display(), e))
            .and_then(|_| { image.dirty = true; Ok(()) })
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn save(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, file: PathBuf);
    with_image_id(id, |image| {
        pdpfs::ops::save_image(image.fs.block_device().physical_device(), &file)
            .map_err(|e| format!("Couldn't save {}: {}", file.display(), e))
            .and_then(|_| { image.dirty = false; Ok(()) })
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn convert(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, file: PathBuf, image_type: pdpfs::ops::ImageType);
    with_image_id(id, |image| {
        pdpfs::ops::convert(image.fs.block_device(), image_type, &file)
            .map_err(|e| format!("Couldn't save {}: {}", file.display(), e))
            .and_then(|_| { Ok(()) })
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn filesystem_name(mut cx: FunctionContext) -> JsResult<JsString> {
    js_args!(&mut cx, id: u32);
    let fs_name = with_image_id(id, |image| {
        Result::<_,String>::Ok(cx.string(&image.fs.filesystem_name()))
    }).into_jserr(&mut cx)?;
    Ok(fs_name)
}

use pdpfs::ops::strum::VariantNames;
fn device_types<'a>(mut cx: FunctionContext<'a>) -> JsResult<JsArray> {
    let v = pdpfs::ops::DeviceType::VARIANTS.iter().map(|name| {
        let dtype = pdpfs::ops::DeviceType::try_from(*name).unwrap(/*would be weird if the variants and the enum didn't match*/);
        let obj = cx.empty_object();
        obj_set_string(&mut cx, &obj, "name", name)?;
        obj_set_number(&mut cx, &obj, "bytes", dtype.geometry().bytes() as u32)?;
        obj_set_number(&mut cx, &obj, "cylinders", dtype.geometry().cylinders as u32)?;
        obj_set_number(&mut cx, &obj, "heads", dtype.geometry().heads as u32)?;
        obj_set_number(&mut cx, &obj, "sectors", dtype.geometry().sectors as u32)?;
        obj_set_number(&mut cx, &obj, "sector_size", dtype.geometry().sector_size as u32)?;
        Ok(obj.upcast())
    }).collect::<NeonResult<Vec<Handle<JsValue>>>>()?;

    vec_to_array(&mut cx, &v)
}

fn image_types(mut cx: FunctionContext) -> JsResult<JsArray> {
    vec_to_array(&mut cx, &pdpfs::ops::ImageType::VARIANTS)
}

fn filesystems(mut cx: FunctionContext) -> JsResult<JsArray> {
    vec_to_array(&mut cx, &pdpfs::ops::FileSystemType::VARIANTS)
}
