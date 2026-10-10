//! The widget class of a layout element, parsed once at load.
//!
//! `sprites_reporting_bg` used to compare `e.class` against ~60 literals for every widget on every
//! frame, and the three lists "which classes take a click / can hover / take focus on a press"
//! were written out separately. [`WidgetClass::from_class`] does the string compare once per
//! element (the match is exactly the old draw `match`, arm for arm, so a class this file does not
//! name is [`Other`](WidgetClass::Other) and takes the old `_` arm); the per-class facts live
//! here as methods. The comparison is **exact** — the old `match` was case sensitive, and the call
//! sites that compare with `eq_ignore_ascii_case` still do so on the string.

/// A layout element's class (`id=` of its section), for the classes the front end tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WidgetClass {
    /// `REAL.BLACK*` — the screen's own fill (prefix match)
    ScreenFill,
    GouraudBox,
    GouraudIndicatorBar,
    GlintyRectangle,
    Panel,
    Car3d,
    DeadZone,
    InputBox,
    ChatInputBox,
    ChatDisplayBox,
    CharVectAccessorInputBox,
    ControllerSetup,
    ListBox,
    RadioGroup,
    TrackSmall2dSpline,
    PrizeRaceMap,
    FactoryMap,
    PlayerPicture,
    MiniPlayerPicture,
    Badge,
    MiniBadge,
    FactoryPic,
    Hud,
    MovieBox,
    MovieCircle,
    AccountThing,
    AnimShowcasePicture,
    ChroniclePictureIcon,
    ShowcasePicture,
    ChroniclePicture,
    CarLogo,
    KnockoutTrackList,
    ChronicleMainBoxControl,
    AnimatingPicture,
    Header,
    SubHeader,
    RealLabel,
    Picture,
    NonLocalizedLabel,
    ExtraCircle,
    OutlineRectangle,
    Rectangle,
    BeveledOptionBox,
    GlintyLine,
    CheckBox,
    CarSettingSlider,
    ChronicleSlider,
    ComboBoxSlider,
    ColorBox,
    GotoButton,
    BottomBarButton,
    GraphicTabButton,
    TabButton,
    CameraSelect,
    CameraPicture,
    /// `NFS5.CARSETTINGBUTTON` — only named by [`takes_focus_on_press`](Self::takes_focus_on_press)
    CarSettingButton,
    /// every other class: the old `match`'s `_` arm
    Other,
}

impl WidgetClass {
    /// The class of an element whose `id` is `class` (exact, case sensitive).
    pub(super) fn from_class(class: &str) -> Self {
        use WidgetClass::*;
        match class {
            c if c.starts_with("REAL.BLACK") => ScreenFill,
            "NFS5.GOURAUDBOX" => GouraudBox,
            "NFS5.GouraudIndicatorBar" => GouraudIndicatorBar,
            "NFS5.GlintyRectangle" => GlintyRectangle,
            "NFS5.PANEL" => Panel,
            "NFS5.3DCAR" => Car3d,
            "NFS5.DEADZONE" => DeadZone,
            "NFS5.INPUTBOX" => InputBox,
            "NFS5.CHATINPUTBOX" => ChatInputBox,
            "NFS5.CHATDISPLAYBOX" => ChatDisplayBox,
            "NFS5.CHARVECTACCESSORINPUTBOX" => CharVectAccessorInputBox,
            "NFS5.CONTROLLERSETUP" => ControllerSetup,
            "NFS5.ListBox" => ListBox,
            "NFS5.RADIOGROUP" => RadioGroup,
            "NFS5.TrackSmall2dSpline" => TrackSmall2dSpline,
            "NFS5.PRIZERACEMAP" => PrizeRaceMap,
            "NFS5.FACTORYMAP" => FactoryMap,
            "NFS5.PLAYERPICTURE" => PlayerPicture,
            "NFS5.MINIPLAYERPICTURE" => MiniPlayerPicture,
            "NFS5.BADGE" => Badge,
            "NFS5.MINIBADGE" => MiniBadge,
            "NFS5.FACTORYPIC" => FactoryPic,
            "NFS5.HUD" => Hud,
            "NFS5.MOVIEBOX" => MovieBox,
            "NFS5.MOVIECIRCLE" => MovieCircle,
            "NFS5.ACCOUNTTHING" => AccountThing,
            "NFS5.ANIMSHOWCASEPICTURE" => AnimShowcasePicture,
            "NFS5.ChroniclePictureIcon" => ChroniclePictureIcon,
            "NFS5.ShowcasePicture" => ShowcasePicture,
            "NFS5.ChroniclePicture" => ChroniclePicture,
            "NFS5.CarLogo" => CarLogo,
            "NFS5.KNOCKOUTTRACKLIST" => KnockoutTrackList,
            "NFS5.ChronicleMainBoxControl" => ChronicleMainBoxControl,
            "NFS5.AnimatingPicture" => AnimatingPicture,
            "NFS5.HEADER" => Header,
            "NFS5.SUBHEADER" => SubHeader,
            "REAL.LABEL" => RealLabel,
            "NFS5.PICTURE" => Picture,
            "NFS5.NonLocalizedLabel" => NonLocalizedLabel,
            "NFS5.ExtraCircle" => ExtraCircle,
            "NFS5.OutlineRectangle" => OutlineRectangle,
            "NFS5.Rectangle" => Rectangle,
            "NFS5.BEVELEDOPTIONBOX" => BeveledOptionBox,
            "NFS5.GLINTYLINE" => GlintyLine,
            "NFS5.CheckBox" => CheckBox,
            "NFS5.CarSettingSlider" => CarSettingSlider,
            "NFS5.CHRONICLESLIDER" => ChronicleSlider,
            "NFS5.ComboBoxSlider" => ComboBoxSlider,
            "NFS5.ColorBox" => ColorBox,
            "NFS5.GOTOBUTTON" => GotoButton,
            "NFS5.BOTTOMBARBUTTON" => BottomBarButton,
            "NFS5.GRAPHICTABBUTTON" => GraphicTabButton,
            "NFS5.TABBUTTON" => TabButton,
            "NFS5.CAMERASELECT" => CameraSelect,
            "NFS5.CAMERAPICTURE" => CameraPicture,
            "NFS5.CARSETTINGBUTTON" => CarSettingButton,
            _ => Other,
        }
    }

    /// The three text-entry classes: they react to a click without ever having a `[state.hover]`
    /// (the release goes to their slot 9, 0x4CCB70 / 0x4C2220, which takes the focus).
    pub(super) fn takes_click(self) -> bool {
        matches!(self, Self::InputBox | Self::ChatInputBox | Self::CharVectAccessorInputBox)
    }

    /// The classes a press gives the focus to (`RE/notes/controls.md` §4.40) — the old
    /// `PRESS_FOCUS` list; `can_press_at` additionally needs the template's `[state.focused]`.
    pub(super) fn takes_focus_on_press(self) -> bool {
        matches!(self, Self::CarSettingButton | Self::DeadZone | Self::GouraudIndicatorBar)
    }
}
