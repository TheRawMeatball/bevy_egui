use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use bevy::{
    asset::{AssetId, AssetIndex},
    ecs::system::Resource,
    reflect::{FromReflect, GetField},
    render::texture::Image as BevyImage,
    utils::{nonmax::NonMaxU64, HashMap, Uuid},
};
use egui::{
    generate_loader_id,
    load::{LoadError, TextureLoadResult, TextureLoader, TexturePoll},
    mutex::Mutex,
    ImageSource, TextureFilter, TextureId, TextureOptions, TextureWrapMode,
};

use crate::render_systems::EguiTextureId;

#[derive(Resource, Default, Clone)]
pub struct EguiUserTextures {
    pub(crate) loader: Arc<BevyTextureLoader>,
}

pub(crate) struct LoaderEntry {
    egui_ids: [Option<NonMaxU64>; 12],
    bevy_id: AssetId<BevyImage>,
    pub(crate) size: Option<egui::Vec2>,
}

#[derive(Default)]
pub(crate) struct BevyTextureLoader {
    pub(crate) map: Mutex<HashMap<String, LoaderEntry>>,
    pub(crate) new: Mutex<Vec<(String, AssetId<BevyImage>)>>,
    user_id_counter: AtomicU64,
}

pub trait AsImageSource {
    fn into_uri(self) -> String;
    fn as_source(self) -> ImageSource<'static>;
}

impl<T: Into<AssetId<BevyImage>>> AsImageSource for T {
    fn into_uri(self) -> String {
        let id = self.into();
        match id {
            AssetId::Index { index, .. } => {
                // TODO: fix this once bevy adds a to_bits/from_bits for AssetIndex
                let gen = *index.get_field::<u32>("generation").unwrap() as u64;
                let index = *index.get_field::<u32>("index").unwrap() as u64;
                let combined = (gen << 32) | index;
                format!("bevy://index/{}", combined)
            }
            AssetId::Uuid { uuid } => format!("bevy://uuid/{uuid}"),
        }
    }
    fn as_source(self) -> ImageSource<'static> {
        ImageSource::Uri(self.into_uri().into())
    }
}

impl TextureLoader for BevyTextureLoader {
    fn id(&self) -> &str {
        generate_loader_id!(BevyTextureLoader)
    }

    fn load(
        &self,
        _ctx: &egui::Context,
        uri: &str,
        texture_options: TextureOptions,
        _size_hint: egui::SizeHint,
    ) -> TextureLoadResult {
        if !uri.starts_with("bevy://") {
            return TextureLoadResult::Err(LoadError::NotSupported);
        }
        let key = texture_option_bits(texture_options);

        // check if the image already has an egui id assigned
        if let Some(entry) = self.map.lock().get_mut(uri) {
            let id = entry.egui_ids[key].get_or_insert_with(|| {
                NonMaxU64::new(self.user_id_counter.fetch_add(1, Ordering::Relaxed)).unwrap()
            });

            return if let Some(size) = entry.size {
                TextureLoadResult::Ok(TexturePoll::Ready {
                    texture: (TextureId::User((*id).into()), size).into(),
                })
            } else {
                TextureLoadResult::Ok(TexturePoll::Pending { size: None })
            };
        }

        // We've checked the first seven bytes already
        let uri_remainder = &uri[7..];

        let bevy_id = if uri_remainder.starts_with("uuid/") {
            let uuid = &uri_remainder[5..];
            let uuid = uuid.parse::<Uuid>().map_err(|_| LoadError::NotSupported)?;
            AssetId::from(uuid)
        } else if uri_remainder.starts_with("index/") {
            let index = &uri_remainder[6..];
            let index = index.parse::<u64>().map_err(|_| LoadError::NotSupported)?;
            let generation = index >> 32;
            let index = (index << 32) >> 32;
            let mut reflect = bevy::reflect::DynamicStruct::default();
            reflect.insert("generation", generation as u32);
            reflect.insert("index", index as u32);
            AssetId::from(AssetIndex::from_reflect(&reflect).unwrap())
        } else {
            // malformed uri
            return TextureLoadResult::Err(LoadError::NotSupported);
        };

        let mut egui_ids: [Option<NonMaxU64>; 12] = Default::default();

        egui_ids[key] =
            Some(NonMaxU64::new(self.user_id_counter.fetch_add(1, Ordering::Relaxed)).unwrap());

        self.map.lock().insert(
            uri.into(),
            LoaderEntry {
                egui_ids,
                bevy_id,
                size: None,
            },
        );

        self.new.lock().push((uri.into(), bevy_id));

        TextureLoadResult::Ok(TexturePoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        self.map.lock().remove(uri);
    }

    fn forget_all(&self) {
        self.map.lock().clear();
    }

    fn byte_size(&self) -> usize {
        0
    }
}

impl LoaderEntry {
    pub(crate) fn ids(
        &self,
    ) -> impl Iterator<Item = (AssetId<BevyImage>, EguiTextureId, usize)> + '_ {
        self.egui_ids
            .iter()
            .enumerate()
            .filter_map(|(i, id)| id.map(u64::from).map(EguiTextureId::User).zip(Some(i)))
            .map(|(egui_id, options)| (self.bevy_id, egui_id, options))
    }
}

fn texture_option_bits(options: TextureOptions) -> usize {
    let magnification = match options.magnification {
        TextureFilter::Nearest => 0,
        TextureFilter::Linear => 1,
    };
    let minification = match options.minification {
        TextureFilter::Nearest => 0,
        TextureFilter::Linear => 2,
    };
    let wrap_mode = match options.wrap_mode {
        TextureWrapMode::ClampToEdge => 0,
        TextureWrapMode::Repeat => 4,
        TextureWrapMode::MirroredRepeat => 8,
    };

    magnification | minification | wrap_mode
}

pub(crate) fn decode_texture_option_bits(bits: usize) -> TextureOptions {
    fn get_filter(bit: bool) -> TextureFilter {
        if bit {
            TextureFilter::Linear
        } else {
            TextureFilter::Nearest
        }
    }
    TextureOptions {
        magnification: get_filter((bits & 1) > 0),
        minification: get_filter((bits & 2) > 0),
        wrap_mode: if (bits & 4) > 0 {
            TextureWrapMode::Repeat
        } else if (bits & 8) > 0 {
            TextureWrapMode::MirroredRepeat
        } else {
            TextureWrapMode::ClampToEdge
        },
    }
}
