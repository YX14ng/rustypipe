use serde::Deserialize;
use serde_with::{serde_as, DefaultOnError, VecSkipError};

use crate::serializer::text::{AttributedText, Text, TextComponents};

use super::{
    music_item::{
        Button, ItemSection, MusicContentsRenderer, MusicItemMenuEntry, MusicMicroformat,
        MusicThumbnailRenderer,
    },
    url_endpoint::OnTapWrap,
    ContentsRenderer, SectionList, Tab,
};

/// Response model for YouTube Music playlists and albums
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MusicPlaylist {
    pub contents: Option<Contents>,
    pub header: Option<Header>,
    #[serde(default)]
    pub microformat: MusicMicroformat,
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Contents {
    SingleColumnBrowseResultsRenderer(ContentsRenderer<Tab<PlSectionList>>),
    #[serde(rename_all = "camelCase")]
    TwoColumnBrowseResultsRenderer {
        /// List content
        secondary_contents: PlSectionList,
        /// Header
        #[serde_as(as = "VecSkipError<_>")]
        tabs: Vec<Tab<SectionList<Header>>>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlSectionList {
    /// Includes a continuation token for fetching recommendations
    pub section_list_renderer: MusicContentsRenderer<ItemSection>,
}

#[derive(Debug)]
pub(crate) struct Header {
    pub music_detail_header_renderer: HeaderRenderer,
}

/// The signed-in user's own playlists wrap their header in a
/// `musicEditablePlaylistDetailHeaderRenderer`, which holds the usual header under `header`.
/// Without reading it, those playlists were mapped as albums and failed.
impl<'de> Deserialize<'de> for Header {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        enum Wrapped {
            #[serde(alias = "musicResponsiveHeaderRenderer")]
            MusicDetailHeaderRenderer(HeaderRenderer),
            MusicEditablePlaylistDetailHeaderRenderer(Editable),
        }

        #[derive(Deserialize)]
        struct Editable {
            header: Box<Header>,
        }

        Ok(match Wrapped::deserialize(deserializer)? {
            Wrapped::MusicDetailHeaderRenderer(renderer) => Header {
                music_detail_header_renderer: renderer,
            },
            Wrapped::MusicEditablePlaylistDetailHeaderRenderer(editable) => *editable.header,
        })
    }
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeaderRenderer {
    #[serde_as(as = "Text")]
    pub title: String,
    /// Content type + Channel/Artist + Year.
    /// Missing on artist_tracks view.
    ///
    /// `"Playlist", " • ", <"Best Music">, " • ", "2022"`
    ///
    /// `"Album", " • ", <"Helene Fischer">, " • ", "2021"`
    #[serde(default)]
    pub subtitle: TextComponents,
    /// Playlist/album description. May contain hashtags which are
    /// displayed as search links on the YouTube website.
    pub description: Option<Description>,
    /// Playlist thumbnail / album cover.
    /// Missing on artist_tracks view.
    #[serde(default)]
    pub thumbnail: MusicThumbnailRenderer,
    /// Channel (only on TwoColumnBrowseResultsRenderer)
    pub strapline_text_one: Option<TextComponents>,
    /// Number of tracks + playtime.
    /// Missing on artist_tracks view.
    ///
    /// `"64 songs", " • ", "3 hours, 40 minutes"`
    ///
    /// `"1B views", " • ", "200 songs", " • ", "6+ hours"`
    ///
    /// Empty (`{}`) on mixes made for the user, like "My Supermix".
    #[serde(default)]
    #[serde_as(as = "DefaultOnError<Text>")]
    pub second_subtitle: Vec<String>,
    /// Channel (newer data model)
    #[serde(default)]
    #[serde_as(as = "DefaultOnError")]
    pub facepile: Option<AvatarStackViewModelWrap>,
    #[serde(default)]
    #[serde_as(as = "DefaultOnError")]
    pub menu: Option<HeaderMenu>,
    #[serde(default)]
    #[serde_as(as = "VecSkipError<_>")]
    pub buttons: Vec<HeaderMenu>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Description {
    #[serde(rename_all = "camelCase")]
    Shelf {
        music_description_shelf_renderer: DescriptionShelf,
    },
    Text(TextComponents),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DescriptionShelf {
    pub description: TextComponents,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeaderMenu {
    pub menu_renderer: HeaderMenuRenderer,
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HeaderMenuRenderer {
    #[serde(default)]
    #[serde_as(as = "VecSkipError<_>")]
    pub top_level_buttons: Vec<Button>,
    #[serde_as(as = "VecSkipError<_>")]
    pub items: Vec<MusicItemMenuEntry>,
}

impl From<Description> for TextComponents {
    fn from(value: Description) -> Self {
        match value {
            Description::Text(v) => v,
            Description::Shelf {
                music_description_shelf_renderer,
            } => music_description_shelf_renderer.description,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AvatarStackViewModelWrap {
    pub avatar_stack_view_model: AvatarStackViewModel,
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AvatarStackViewModel {
    // #[serde(default)]
    // pub avatars: Vec<AvatarViewModel>,
    #[serde_as(as = "AttributedText")]
    pub text: String,
    pub renderer_context: AvatarStackRendererContext,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AvatarStackRendererContext {
    pub command_context: Option<OnTapWrap>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_header_with_an_empty_second_subtitle() {
        let header: Header = serde_json::from_value(serde_json::json!({
            "musicResponsiveHeaderRenderer": {
                "title": { "runs": [{ "text": "New Release Mix" }] },
                "subtitle": { "runs": [{ "text": "Mix" }, { "text": " • " }, { "text": "2026" }] },
                "secondSubtitle": {},
                "facepile": { "avatarStackViewModel": { "text": { "content": "YouTube Music" } } },
            }
        }))
        .unwrap();
        assert_eq!(header.music_detail_header_renderer.title, "New Release Mix");
        assert!(header
            .music_detail_header_renderer
            .second_subtitle
            .is_empty());
    }
}
