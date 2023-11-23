#[macro_use]
extern crate lazy_static;

mod make_neon_usable;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;
use std::sync::Mutex;

use neon::prelude::*;

use make_neon_usable::*;
use pdpfs::fs::{FileSystem, Timestamp};
use pdpfs::fs::rt11::RT11FS;
use pdpfs::block::BlockDevice;

#[neon::main]
fn main(mut cx: ModuleContext) -> NeonResult<()> {
    cx.export_function("open_image", open_image)?;
    cx.export_function("get_directory_entries", get_directory_entries)?;
    cx.export_function("extract_to_path", extract_to_path)?;
    cx.export_function("cp_into_image", cp_into_image)?;
    cx.export_function("mv", mv)?;
    cx.export_function("rm", rm)?;
    cx.export_function("save", save)?;
    cx.export_function("convert", convert)?;
    Ok(())
}

struct Image {
    fs: Box<dyn FileSystem<BlockDevice=Box<dyn BlockDevice>>>,
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
        let e: Box<dyn std::error::Error> = format!("Bad ID").into();
        return Err(Error::Std(e));
    };
    func(image).map_err(|e| Error::from(e))
}

fn open_image(mut cx: FunctionContext) -> JsResult<JsNumber> {
    js_args!(&mut cx, image_file: PathBuf);

    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let fs = Box::new(RT11FS::new(pdpfs::ops::open_device(&Path::new(&image_file)).expect("phys image bad")).expect("fs bad"));

    IMAGES.lock().unwrap().insert(id, Image { fs });

    Ok(cx.number(id))
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

fn extract_to_path(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, path: PathBuf);
    with_image_id(id, |image| -> Result<(),String> {
        std::fs::create_dir_all(&path).map_err(|e| format!("Could't create directory: {}", e))?;
        for entry in image.fs.read_dir("/")
            .map_err(|e| format!("Could't read directory: {}", e))?
        {
            std::fs::write(path.join(&entry.file_name()), image.fs.read_file(&entry.file_name()).unwrap().as_bytes())
                .map_err(|e| format!("Could't write {}: {}", path.join(&entry.file_name()).display(), e))?;
        }
        Ok(())
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn cp_into_image(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, path: PathBuf);
    with_image_id(id, |image| {
        pdpfs::ops::cp_into_image(&mut image.fs, &path, Path::new("."))
            .map_err(|e| format!("Could't write {}: {}", path.display(), e))
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn mv(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, src: PathBuf, dest: PathBuf, overwrite_dest: bool);
    with_image_id(id, |image| {
        pdpfs::ops::mv(&mut image.fs, &src, &dest, overwrite_dest)
            .map_err(|e| format!("mv from {} to {} failed: {}", src.display(), dest.display(), e))
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn rm(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, file: PathBuf);
    with_image_id(id, |image| {
        pdpfs::ops::rm(&mut image.fs, &file)
            .map_err(|e| format!("rm failed for {}: {}", file.display(), e))
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn save(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, file: PathBuf);
    with_image_id(id, |image| {
        pdpfs::ops::save_image(image.fs.block_device().physical_device(), &file)
            .map_err(|e| format!("Couldn't save {}: {}", file.display(), e))
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

fn convert(mut cx: FunctionContext) -> JsResult<JsNull> {
    js_args!(&mut cx, id: u32, file: PathBuf, image_type: pdpfs::ops::ImageType);
    with_image_id(id, |image| {
        pdpfs::ops::convert(image.fs.block_device(), image_type, &file)
            .map_err(|e| format!("Couldn't save {}: {}", file.display(), e))
    }).into_jserr(&mut cx)?;
    Ok(cx.null())
}

