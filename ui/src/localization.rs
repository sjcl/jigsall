//! UI-only localization. Catalogs and fonts are embedded, independent of cwd.
use bevy::prelude::*;
use fluent_bundle::{concurrent::FluentBundle, FluentArgs, FluentResource, FluentValue};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

/// A supported BCP-47 ID from the embedded catalog registry, never a platform code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Locale(&'static str);

impl Locale {
    pub const EN_US: Self = Self("en-US");
    pub const JA: Self = Self("ja");

    pub fn id(self) -> &'static str {
        self.0
    }

    pub fn available() -> impl Iterator<Item = Self> {
        CATALOGS.iter().map(|(id, _)| Self(id))
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::available().find(|locale| locale.id().eq_ignore_ascii_case(id))
    }

    /// Exact match first, then the base language (e.g. ja-JP -> ja, en-GB -> en-US).
    pub fn from_language_tag(tag: &str) -> Option<Self> {
        let tag = tag.split(['.', '@']).next()?.replace('_', "-");
        Self::from_id(&tag).or_else(|| {
            let language = tag.split('-').next()?;
            Self::available().find(|locale| {
                locale
                    .id()
                    .split('-')
                    .next()
                    .is_some_and(|base| base.eq_ignore_ascii_case(language))
            })
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LanguagePreference {
    #[default]
    Auto,
    Locale(Locale),
}

impl Serialize for LanguagePreference {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::Auto => "auto",
            Self::Locale(locale) => locale.id(),
        })
    }
}

impl<'de> Deserialize<'de> for LanguagePreference {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        if id == "auto" {
            return Ok(Self::Auto);
        }
        Locale::from_id(&id)
            .map(Self::Locale)
            .ok_or_else(|| serde::de::Error::custom(format!("unsupported locale ID: {id}")))
    }
}

/// Insert a future Steam adapter here; return internal Locale values only.
pub trait PlatformLocaleProvider: Send + Sync + 'static {
    fn current_locale(&self) -> Option<Locale>;
}

pub struct OsLocaleProvider;
impl PlatformLocaleProvider for OsLocaleProvider {
    fn current_locale(&self) -> Option<Locale> {
        sys_locale::get_locales().find_map(|tag| Locale::from_language_tag(&tag))
    }
}

/// Arguments keep Fluent out of screen code and allow numeric plural selectors.
pub enum Argument<'a> {
    Text(&'a str),
    Number(f64),
}
impl<'a> From<&'a str> for Argument<'a> {
    fn from(value: &'a str) -> Self {
        Self::Text(value)
    }
}
macro_rules! numeric_arguments {
    ($($ty:ty),*) => { $(impl From<$ty> for Argument<'_> {
        fn from(value: $ty) -> Self { Self::Number(value as f64) }
    })* };
}
numeric_arguments!(usize, u32, u64, f32, f64);

#[derive(Resource)]
pub struct Localization {
    locale: Locale,
    bundles: HashMap<Locale, FluentBundle<FluentResource>>,
    provider: Box<dyn PlatformLocaleProvider>,
    reported: Mutex<HashSet<String>>,
}

impl Default for Localization {
    fn default() -> Self {
        Self::new(Box::new(OsLocaleProvider))
    }
}

fn bundle(locale: Locale, source: &str) -> FluentBundle<FluentResource> {
    let locales = locale.id().parse().into_iter().collect();
    let mut bundle = FluentBundle::new_concurrent(locales);
    let resource = match FluentResource::try_new(source.to_owned()) {
        Ok(resource) => resource,
        Err((resource, errors)) => {
            warn!("Invalid Fluent catalog {}: {errors:?}", locale.id());
            resource
        }
    };
    if let Err(errors) = bundle.add_resource(resource) {
        warn!("Duplicate Fluent keys in {}: {errors:?}", locale.id());
    }
    // Both shipped languages are LTR. Avoid isolation marks in numerical HUD values.
    bundle.set_use_isolating(false);
    bundle
}

impl Localization {
    pub fn new(provider: Box<dyn PlatformLocaleProvider>) -> Self {
        let mut localization = Self {
            locale: Locale::EN_US,
            bundles: CATALOGS
                .iter()
                .map(|(id, source)| (Locale(id), bundle(Locale(id), source)))
                .collect(),
            provider,
            reported: Mutex::default(),
        };
        localization.set_preference(LanguagePreference::Auto);
        localization
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }

    pub fn set_preference(&mut self, preference: LanguagePreference) {
        self.locale = match preference {
            LanguagePreference::Locale(locale) => locale,
            LanguagePreference::Auto => self.provider.current_locale().unwrap_or(Locale::EN_US),
        };
    }

    pub fn text(&self, key: &str) -> String {
        self.format(key, &[])
    }

    pub fn native_name(&self, locale: Locale) -> String {
        self.resolve(locale, "language-native-name", None)
            .unwrap_or_else(|| locale.id().into())
    }

    pub fn format(&self, key: &str, arguments: &[(&str, Argument<'_>)]) -> String {
        let mut args = FluentArgs::new();
        for (name, value) in arguments {
            args.set(
                *name,
                match value {
                    Argument::Text(value) => FluentValue::from(*value),
                    Argument::Number(value) => FluentValue::from(*value),
                },
            );
        }
        self.resolve(self.locale, key, Some(&args))
            .or_else(|| {
                (self.locale != Locale::EN_US)
                    .then(|| self.resolve(Locale::EN_US, key, Some(&args)))
                    .flatten()
            })
            .unwrap_or_else(|| format!("[{key}]"))
    }

    fn resolve(&self, locale: Locale, key: &str, args: Option<&FluentArgs<'_>>) -> Option<String> {
        let bundle = self.bundles.get(&locale)?;
        let Some(pattern) = bundle.get_message(key).and_then(|message| message.value()) else {
            self.report(format!("Missing Fluent key {}:{key}", locale.id()));
            return None;
        };
        let mut errors = vec![];
        let value = bundle
            .format_pattern(pattern, args, &mut errors)
            .into_owned();
        if errors.is_empty() {
            Some(value)
        } else {
            self.report(format!(
                "Fluent format error {}:{key}: {errors:?}",
                locale.id()
            ));
            None
        }
    }

    fn report(&self, message: String) {
        if let Ok(mut reported) = self.reported.lock() {
            if reported.insert(message.clone()) {
                warn!("{message}");
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
