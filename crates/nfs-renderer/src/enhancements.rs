//! The enhanced game's switches (`RE/notes/ENHANCEMENTS.md`, `REMASTER-PLAN.md`).
//!
//! **Not the original.** Everything an enhancement does is marked `ENHANCEMENT E<n>` in code and
//! is gated on one switch of [`Enhancements`]. The set of switches is [`REGISTRY`]; adding an
//! enhancement is one [`Entry`] there plus one [`Switch`] constant beside it.
//!
//! **The guard.** [`Enhancements::ORIGINAL`] (every level 0) is the original's code path: a GPU-side
//! branch that tests a switch is not entered, so the targets, formats, sample counts and passes
//! are `master`'s. The set reaches the GPU side through the renderer's command list
//! ([`crate::Renderer::set_enhancements`], the same path as `set_gamma`), which is also what the
//! render thread (D5) executes, in call order, at the start of the frame it belongs to. The core
//! starts with [`Enhancements::ORIGINAL`] until the game sets it.
//!
//! **Levels.** A switch is not a bool but a small *level*: index 0 is always "off" (= the original),
//! the others are the entry's choices ([`Entry::levels`], with captions for the menu's combo); the
//! shipped default of an entry is its own [`Entry::default_level`] (MSAA: 4x, not 8x). A plain
//! on/off switch is just the levels `[off, on]`. [`Enhancements::is_on`] is `level > 0`.
//!
//! The struct is a fixed array of levels rather than one field per switch because it is `Copy`
//! (the scene hand-off `..*self`) and a new switch must be a registry entry, not a struct edit.

/// One switch's registry row.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// `E1` — the token of `NFS_ENH=E1,E3`, the key in `enhancements.json` and the `ENHANCEMENT E1`
    /// marker in code
    pub token: &'static str,
    /// the menu row's caption, `[English, Russian]`
    pub name: [&'static str; 2],
    /// the one-line help of the row, `[English, Russian]`
    pub help: [&'static str; 2],
    /// the choices, `[English, Russian]` captions in the combo's order; index 0 is always "off"
    /// (the original), so there are at least two and at most [`MAX_LEVELS`]
    pub levels: &'static [[&'static str; 2]],
    /// the level the shipped game starts on (and `NFS_ENH=all` / a bare `E1` give);
    /// it is below `levels.len()`; 0 = off by default (E3, E4, E6: the original look)
    pub default_level: u8,
}

/// Which platforms an entry's backend exists on. Entries stay in the registry on every platform (a `cfg` on an
/// entry would shift the later switch indices per platform) but where the rule says no they are
/// [`Entry::available`] = false: no menu row, `NFS_ENH` / the stored file cannot set them, `all` leaves them 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platforms {
    /// every platform
    All,
    /// macOS only (E33 EDR, E34 MetalFX)
    MacOnly,
}

/// The per-entry platform rules (the switches not listed are on every platform). E37 (FSR 3.1) is on every platform since the
/// vendored shaders run on Metal too (ENH-UPSCALER: the SPD pyramid's read-write `Rg16Float` images became `Rgba16Float`).
const PLATFORM_RULES: &[(Switch, Platforms)] = &[(E33, Platforms::MacOnly), (E34, Platforms::MacOnly)];

impl Entry {
    /// Whether this platform has the entry's backend (a rule matches the entry by its switch's token, so a registry of the tests
    /// with the same token follows it too).
    pub fn available(&self) -> bool {
        match PLATFORM_RULES.iter().find(|(s, _)| s.token() == self.token).map_or(Platforms::All, |r| r.1) {
            Platforms::All => true,
            Platforms::MacOnly => cfg!(target_os = "macos"),
        }
    }
}

/// How many entries the menu shows (the available ones).
pub fn visible_count() -> usize {
    REGISTRY.iter().filter(|e| e.available()).count()
}

/// The registry index of the `k`-th entry the menu shows.
pub fn visible_index(k: usize) -> usize {
    REGISTRY.iter().enumerate().filter(|(_, e)| e.available()).nth(k).map_or(k, |(i, _)| i)
}

/// Index of an [`Entry`] in [`REGISTRY`]; the constants beside the registry are the code's way to
/// ask for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch(pub u8);

impl Switch {
    /// The entry's token (`E1`, ...): the `NFS_ENH` token and the `enhancements.json` key, from [`REGISTRY`].
    pub const fn token(self) -> &'static str {
        REGISTRY[self.0 as usize].token
    }
}

/// The captions of a plain on/off switch: levels `[off, on]`.
pub const ON_OFF: &[[&str; 2]] = &[["off", "выкл"], ["on", "вкл"]];

// To add one: append `Entry { token: "E45", .. }` here and `E45 = 44,` to `switches!` below (a mismatch does not compile).
// Menu order. Stage 1 closed the framework; ENH-VAL added the levels and registered E1..E6 — their
// GPU side comes later, so for now no level changes anything on screen.
/// Every enhancement of the branch, in menu order.
pub const REGISTRY: &[Entry] = &[
    Entry {
        token: "E1",
        name: ["anti-aliasing (MSAA)", "сглаживание (MSAA)"],
        help: ["smooths jagged edges", "сглаживает рваные края"],
        levels: &[["off", "выкл"], ["2x", "2x"], ["4x", "4x"], ["8x", "8x"]],
        default_level: 2,
    },
    Entry {
        token: "E2",
        name: ["anisotropic filtering", "анизотропная фильтрация"],
        help: ["sharper textures at a slant", "чёткие текстуры под углом"],
        levels: &[["off", "выкл"], ["4x", "4x"], ["8x", "8x"], ["16x", "16x"]],
        default_level: 3,
    },
    Entry {
        token: "E3",
        name: ["render scale", "масштаб рендера"],
        help: ["off = the original 1:1. Not in effect while an upscaler (MetalFX / FSR 3.1) is on: it sets the internal size itself", "выкл = как в оригинале 1:1. Не действует, пока включен апскейлер (MetalFX / FSR 3.1): он сам задает внутренний размер"],
        levels: &[["off (100 %)", "выкл (100 %)"], ["50 %", "50 %"], ["75 %", "75 %"], ["125 %", "125 %"], ["150 %", "150 %"], ["200 %", "200 %"]],
        default_level: 0,
    },
    Entry {
        token: "E4",
        name: ["field of view (FOV)", "угол обзора (FOV)"],
        help: ["a wider view than the original", "шире обзор, чем в оригинале"],
        levels: &[["off (original)", "выкл (оригинал)"], ["+5 deg", "+5°"], ["+10 deg", "+10°"], ["+15 deg", "+15°"], ["+20 deg", "+20°"]],
        default_level: 0,
    },
    Entry {
        token: "E5",
        name: ["draw distance", "дальность прорисовки"],
        help: ["objects stay visible farther", "объекты видны дальше"],
        levels: &[["off (original)", "выкл (оригинал)"], ["+25 %", "+25 %"], ["+50 %", "+50 %"], ["+100 %", "+100 %"]],
        default_level: 2,
    },
    Entry {
        token: "E6",
        name: ["frame output (vsync)", "вывод кадров (vsync)"],
        help: ["mailbox: latest frame, short queue, no tearing (not offered by Metal on macOS: then = off); immediate: no vsync, tearing, the front end is capped at 120 fps", "mailbox: свежий кадр, короткая очередь, без разрывов (на macOS Metal его нет: тогда = выкл); immediate: без vsync, возможны разрывы, меню не быстрее 120 кадров/с"],
        levels: &[["off (as now)", "выкл (как сейчас)"], ["mailbox (low lag)", "mailbox (мал. задержка)"], ["immediate (no vsync)", "immediate (без vsync)"]],
        default_level: 0,
    },
    Entry {
        token: "E7",
        name: ["HDR and tone mapping", "HDR и тонемап"],
        help: [
            "the 3D scene in a 16-bit float image, tone-mapped to the screen before the gamma ramp; 'as the original' reproduces the original's frame, 'contrasty' is an ACES filmic curve",
            "3D-сцена в 16-битном float-изображении, тонемап на экран до гамма-рампы; «как оригинал» повторяет кадр оригинала, «контрастный» - кинематографическая кривая ACES",
        ],
        levels: &[["off", "выкл"], ["as the original", "как оригинал"], ["contrasty (ACES)", "контрастный (ACES)"]],
        default_level: 1,
    },
    Entry {
        token: "E8",
        name: ["body clear coat", "лак кузова"],
        help: [
            "the car paint is a base colour under a clear lacquer that reflects the surroundings like the real thing (a live environment map from the player's car, a physically based coat of 4 % reflectance, stronger at the silhouette) and the glass reflects too; the paint keeps its colour. new: a sharp mirror coat; normal: a slight softness; worn: a soft, rubbed-down coat",
            "кузов - это краска под прозрачным лаком, который отражает окружение как настоящий (живая карта окружения из позиции машины игрока, физически точный лак с отражением 4 %, сильнее у силуэта), стёкла тоже отражают; краска сохраняет свой цвет. новый: чёткий зеркальный лак; обычный: лёгкая размытость; потёртый: мягкий, затёртый лак",
        ],
        levels: &[["off (original)", "выкл (оригинал)"], ["new", "новый"], ["normal", "обычный"], ["worn", "потёртый"]],
        default_level: 2,
    },
    Entry {
        token: "E9",
        name: ["sun shadows", "тени от солнца"],
        help: ["cars and roadside objects cast shadows from the sun (cascaded shadow maps); the car's blob shadow stays", "машины и придорожные объекты отбрасывают тени от солнца; пятно под машиной остаётся"],
        levels: &[["off", "выкл"], ["low (1024, 3 cascades)", "низко (1024, 3 каскада)"], ["high (2048, 4 cascades)", "высоко (2048, 4 каскада)"]],
        default_level: 1,
    },
    Entry {
        token: "E12",
        name: ["soft particles", "мягкие частицы"],
        help: [
            "smoke, dust, spray, sparks and weather sprites fade out where they meet the road, walls and cars instead of being cut off by a hard edge (the depth difference decides)",
            "дым, пыль, брызги, искры и осадки плавно исчезают у дороги, стен и машин вместо жёсткого среза (решает разница глубины)",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
    Entry {
        token: "E10",
        name: ["glow (bloom)", "свечение (bloom)"],
        help: [
            "a soft glow around the brightest things: lamps, the sun, neon, shiny highlights; needs 'HDR and tone mapping' on (with it off this does nothing)",
            "мягкое свечение вокруг самого яркого: фонари, солнце, неон, блики; нужен включённый «HDR и тонемап» (при выключенном ничего не делает)",
        ],
        levels: &[["off", "выкл"], ["light", "слабо"], ["medium", "средне"]],
        default_level: 1,
    },
    Entry {
        token: "E11",
        name: ["bright sun (HDR)", "яркое солнце (HDR)"],
        help: [
            "the sun and its lens flare are drawn brighter than white so that the glow picks them up; still follows the original's visibility test; needs 'HDR and tone mapping' on",
            "солнце и его блики рисуются ярче белого, чтобы их подхватило свечение; по-прежнему зависит от проверки видимости оригинала; нужен включённый «HDR и тонемап»",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
    Entry {
        token: "E16",
        name: ["car light from surroundings", "освещение машин от окружения"],
        help: [
            "the cars' ambient light takes its direction and colour from the surroundings (green from grass, warm from asphalt, dark under a bridge) instead of one flat value; its mean brightness stays the original's. Needs body reflections (E8)",
            "рассеянный свет на машинах берёт направление и цвет из окружения (зелёный от травы, тёплый от асфальта, темнее под мостом) вместо одного плоского значения; средняя яркость остаётся оригинальной. Нужны отражения на кузове (E8)",
        ],
        levels: &[["off", "выкл"], ["normal", "обычно"], ["stronger", "сильнее"]],
        default_level: 1,
    },
    Entry {
        token: "E15",
        name: ["headlights (light and shadows)", "фары (свет и тени)"],
        help: [
            "when the lamps are lit (night tracks) the headlights become real spotlights on the road, the scenery and the cars ahead; with shadows the player's lamps also cast shadows; the original's projected beam, the decal and the glare stay",
            "когда фары включены (ночные трассы) они становятся настоящими прожекторами на дорогу, обстановку и машины впереди; с тенями фары игрока ещё и отбрасывают тени; оригинальный проецируемый луч, пятно и блики остаются",
        ],
        levels: &[["off", "выкл"], ["no shadows", "без теней"], ["with shadows", "с тенями"]],
        default_level: 2,
    },
    Entry {
        token: "E13",
        name: ["light shafts", "лучи солнца"],
        help: [
            "volumetric sunlight: rays from the sun through the trees, past cliffs and at tunnel exits (a radial blur of the sky toward the sun, added before the tone map); needs HDR and tone mapping",
            "объёмный свет: лучи солнца сквозь деревья, за скалами и у выхода из тоннеля (радиальное размытие неба к солнцу до тонемапа); нужен HDR и тонемап",
        ],
        levels: &[["off", "выкл"], ["subtle", "слабо"], ["medium", "средне"]],
        default_level: 1,
    },
    Entry {
        token: "E17",
        name: ["water", "вода"],
        help: [
            "the sea, harbour and river water reflects the sky (at night the surroundings, from E8's map) with a Fresnel weight, the sun glints on it and a faint ripple moves the reflection; the original water texture stays underneath",
            "вода моря, гавани и рек отражает небо (ночью - окружение из карты E8) с весом по Френелю, на ней играет блик солнца, лёгкая рябь двигает отражение; исходная текстура воды остаётся под ним",
        ],
        levels: &[["off", "выкл"], ["normal", "обычно"], ["stronger", "сильнее"]],
        default_level: 1,
    },
    Entry {
        token: "E14",
        name: ["corner shading (AO)", "затенение углов (AO)"],
        help: [
            "a light ambient occlusion in creases and corners (under the car, wheel arches, kerbs, wall bases), fading out beyond about 40 m; weak because the track's vertex lighting already carries occlusion",
            "лёгкое затенение в складках и углах (под машиной, арки колёс, бордюры, основания стен), исчезает дальше ~40 м; слабое, так как освещение трассы уже содержит затенение",
        ],
        levels: &[["off", "выкл"], ["weak", "слабо"], ["medium", "средне"]],
        default_level: 1,
    },
    Entry {
        token: "E18",
        name: ["particles in HDR", "частицы в HDR"],
        help: [
            "needs HDR (E7): sparks are drawn far above white so the bloom (E10) makes them glow; tyre smoke, dust, spray and snow take the sun's side (brighter when looking toward the sun) and are darkened in the sun's shadow (E9); the original particle colours stay the base",
            "нужен HDR (E7): искры рисуются намного ярче белого, и свечение (E10) заставляет их светиться; дым шин, пыль, брызги и снег получают свет солнца (ярче при взгляде на солнце) и темнеют в тени от солнца (E9); исходные цвета частиц остаются основой",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
    Entry {
        token: "E19",
        name: ["fog and sky", "туман и небо"],
        help: [
            "the sun's glow in the existing fog (the fog tints warm toward the sun, only on tracks that have both a fog and a sun; away from the sun it is the original's) and dithering of the 8-bit output against banding in smooth gradients (needs HDR and tone mapping)",
            "свечение солнца в существующем тумане (туман теплеет в сторону солнца, только на трассах, где есть и туман, и солнце; вдали от солнца он как в оригинале) и дизеринг 8-битного вывода против полос на плавных градиентах (нужен HDR и тонемап)",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
    Entry {
        token: "E20",
        name: ["lightning lights the scene", "вспышки молнии освещают сцену"],
        help: [
            "on Autobahn and Industrial the lightning flash also lights the track and the cars with a brief bluish-white boost, timed by the original's own flash (the sky bolts are the original's)",
            "на Автобане и в Индастриале вспышка молнии освещает ещё и трассу с машинами коротким голубовато-белым всплеском, по времени точно как вспышка оригинала (молнии на небе остаются оригинальными)",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
    Entry {
        token: "E21",
        name: ["eye adaptation", "адаптация глаза"],
        help: [
            "a slow exposure change when the average brightness jumps (tunnel entry and exit, under bridges): up to half a stop, settling in about a second; steady scenes are untouched. Needs HDR and tone mapping",
            "медленное изменение экспозиции, когда средняя яркость резко меняется (въезд в тоннель и выезд, под мостами): до полустопа, устанавливается примерно за секунду; ровные сцены не затрагиваются. Нужен HDR и тонемап",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
    Entry {
        token: "E23",
        name: ["street lamps light the road", "фонари освещают дорогу"],
        help: [
            "on night tracks (and in tunnels) the street lamps the track already shows as glare sprites become soft point lights: a pool of light on the road under each lamp, light on the walls near it and on the cars passing under it (the nearest 32 lamps; the radius and brightness are authored, the data has none). Night stays night: the rest of the street is as dark as before",
            "на ночных трассах (и в тоннелях) уличные фонари, которые трасса и так показывает световыми пятнами, становятся мягкими точечными источниками: лужа света на дороге под каждым фонарём, свет на стенах рядом и на проезжающих под ним машинах (ближайшие 32 фонаря; радиус и яркость придуманы, в данных их нет). Ночь остаётся ночью: остальная улица так же тёмная, как раньше",
        ],
        levels: &[["off", "выкл"], ["soft", "слабо"], ["normal", "средне"]],
        default_level: 2,
    },
    Entry {
        token: "E24",
        name: ["headlights in tunnels", "фары в тоннелях"],
        help: [
            "on a day track the cars' headlights light up by themselves in a covered stretch (a tunnel, under a deck) and fade out at the exit: the glare, the beam on the road and the light spots. Only the picture changes; the lamp state of the cars, the lights key, replays and network play are not touched",
            "на дневной трассе фары машин сами включаются в закрытом участке (тоннель, под эстакадой) и плавно гаснут на выезде: блики, пучок света на дороге и световые пятна. Меняется только картинка; состояние фар машин, клавиша света, повторы и сетевая игра не затрагиваются",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
    Entry {
        token: "E25",
        name: ["soft shadows when overcast", "рассеянные тени в пасмурную погоду"],
        help: [
            "on the tracks without a sun (Autobahn, Canyon, Foothills, Forest) the shadows of cars and roadside objects fall straight down, very soft and faint, as under a high overcast sky (the original has no shadows there). Needs the sun shadows (E9)",
            "на трассах без солнца (Автобан, Каньон, Предгорья, Лес) тени от машин и придорожных объектов падают прямо вниз, очень мягкие и слабые, как при высокой облачности (в оригинале там теней нет). Нужны тени от солнца (E9)",
        ],
        levels: ON_OFF,
        default_level: 0,
    },
    Entry {
        token: "E22",
        name: ["frame rate limit", "ограничение FPS"],
        help: [
            "holds the game to at most this many frames a second (frame pacing only: the simulation keeps its 64 Hz tick); \"display\" is the screen's own refresh rate (with the frame output on «off» the display's sync already does that); \"auto\" is 60 on the battery and \"display\" on mains power (a laptop stays cool and quiet unplugged). The NFS_FPS_CAP variable still wins",
            "ограничивает игру этим числом кадров в секунду (только темп кадров: симуляция остаётся на своих 64 Гц); «по экрану» - частота обновления самого экрана (при выводе кадров «выкл» это уже делает синхронизация экрана); «авто» - 60 от батареи и «по экрану» от сети (ноутбук без розетки не греется и не шумит). Переменная NFS_FPS_CAP главнее",
        ],
        levels: &[["off", "выкл"], ["30", "30"], ["60", "60"], ["120", "120"], ["240", "240"], ["display", "по экрану"], ["auto (60 on battery)", "авто (60 от батареи)"]],
        // 2026-10-04: «auto» (60 on battery, the display rate on power)
        default_level: 6,
    },
    Entry {
        token: "E27",
        name: ["motion blur", "размытие в движении"],
        help: [
            "a soft streak of the scenery at speed, from the camera's own movement (the car itself stays sharp); subtle, the faster the longer. Taste: off by default. Needs 'HDR and tone mapping' on",
            "мягкая смазка окружения на скорости от самого движения камеры (машина остаётся чёткой); неброская, чем быстрее, тем длиннее. На вкус: по умолчанию выключено. Нужен включённый «HDR и тонемап»",
        ],
        levels: &[["off", "выкл"], ["light", "слабо"], ["medium", "средне"]],
        default_level: 0,
    },
    Entry {
        token: "E28",
        name: ["lens dirt", "грязь на объективе"],
        help: [
            "faint smudges and specks on the lens that light up only when a bright light (the sun, lamps) glows in view, through the bloom; needs 'glow (bloom)' and 'HDR and tone mapping' on",
            "слабые разводы и пылинки на объективе, которые проступают только когда в кадре яркий свет (солнце, фонари), через свечение; нужны включённые «свечение (bloom)» и «HDR и тонемап»",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
    Entry {
        token: "E29",
        name: ["camera shake", "тряска камеры"],
        help: [
            "a small fast shake of the camera on kerbs, rough ground and in hard knocks and landings, from the car's own suspension and collision state; the world shakes with parallax while the cockpit stays with the car (the sound, AI and culling use the unshaken camera; the HUD does not shake)",
            "небольшая быстрая тряска камеры на поребриках, неровной земле, при сильных ударах и приземлениях, по состоянию подвески и столкновений самой машины; мир трясётся с параллаксом, салон остаётся с машиной (звук, ИИ и отсечение используют камеру без тряски; индикаторы не трясутся)",
        ],
        levels: &[["off", "выкл"], ["light", "слабо"], ["medium", "средне"]],
        default_level: 1,
    },
    Entry {
        token: "E26",
        name: ["wet road and rain", "мокрая дорога и дождь"],
        help: [
            "where the original rains (Autobahn, by its own per-slice wetness) the road darkens and shines: the sky and the car lamps reflect in it as soft vertical streaks, gentle puddles form, small rings and splashes play near the camera and the wheel spray is thicker; dry stretches stay dry, transitions are smooth",
            "там, где в оригинале идёт дождь (Автобан, по его собственной влажности участков), дорога темнеет и блестит: небо и фонари машин отражаются в ней мягкими вертикальными полосами, появляются неглубокие лужи, рядом с камерой играют кольца и брызги, брызги из-под колёс гуще; сухие участки остаются сухими, переходы плавные",
        ],
        levels: &[["off", "выкл"], ["on", "вкл"], ["strong", "сильно"]],
        default_level: 1,
    },
    Entry {
        token: "E31",
        name: ["depth of field in replays", "глубина резкости в повторах"],
        help: [
            "while a replay plays, the focus follows the viewed car and the scenery behind and in front of it softens a little, like a camera lens (picture only; needs 'HDR and tone mapping'). In photo mode the depth of field is set there",
            "пока идёт повтор, фокус следует за машиной, а окружение дальше и ближе неё слегка размывается, как в объективе (только картинка; нужен «HDR и тонемап»). В фото-режиме глубина резкости настраивается в нём самом",
        ],
        levels: ON_OFF,
        default_level: 0,
    },
    Entry {
        token: "E32",
        name: ["global illumination", "глобальное освещение"],
        help: [
            "light bounced from the surroundings onto the track, baked once per track (the first race on a track bakes it for some seconds in the background, then it is cached): walls and road edges take the colour of the grass, rock or wall beside them, and a little brightness; in the shade the sky's hue fills in. Day tracks only; the original's own lighting stays the base",
            "свет, отражённый от окружения на трассу, запекается один раз для каждой трассы (в первой гонке на трассе несколько секунд считается в фоне, потом берётся из кэша): стены и края дороги получают цвет травы, скал или стены рядом и немного яркости; в тени проступает оттенок неба. Только дневные трассы; собственное освещение оригинала остаётся основой",
        ],
        levels: &[["off", "выкл"], ["weak", "слабо"], ["medium", "средне"]],
        // 2026-10-04: «средне»
        default_level: 2,
    },
    Entry {
        token: "E35",
        name: ["SMAA smoothing", "сглаживание SMAA"],
        help: [
            "a post pass over the finished picture (SMAA 1x: edges found by brightness, blended along their shape): removes the jaggies MSAA leaves, on foliage, fences, wires far away and shiny glints; the HUD and the mirror are not touched. Costs a little speed",
            "дополнительный проход по готовой картинке (SMAA 1x: границы ищутся по яркости и смягчаются по их форме): убирает зубцы, которые оставляет MSAA, на листве, заборах, далёких проводах и бликах; индикаторы и зеркало не затрагиваются. Немного замедляет",
        ],
        levels: ON_OFF,
        // 2026-10-03: off by default — over MSAA 4x it changes 0.01-0.65 % of the frame and costs up to ~1 ms
        // in dense foliage (the fans); a menu option
        default_level: 0,
    },
    Entry {
        token: "E36",
        name: ["haze and headlight beams", "дымка и лучи фар"],
        help: [
            "where the air is not clean (the track's own fog, rain, a tunnel) the headlight cones and the tunnel lamps show in it as soft light: the lit beam of your car and the cars ahead, a faint warm haze round the lamps inside a tunnel (a screen-space ray march added before the tone map; nothing on a clear dry open road). Needs HDR and tone mapping (E7) and the headlights as spotlights (E15) for the beams",
            "там, где воздух не чистый (туман трассы, дождь, тоннель), конусы фар и лампы тоннеля видны в нём мягким светом: луч ваших фар и машин впереди, лёгкая тёплая дымка вокруг ламп в тоннеле (ray march в экранном пространстве, добавляется до тонемапа; на чистой сухой открытой дороге ничего). Нужны HDR и тонемап (E7) и фары-прожекторы (E15) для лучей",
        ],
        levels: &[["off", "выкл"], ["subtle", "слабо"], ["medium", "средне"]],
        // 2026-10-04: «средне»
        default_level: 2,
    },
    // ENHANCEMENT E34: MetalFX (macOS only: on the other platforms the level is forced to 0, see `Entry::available`).
    // ENH-UPSCALER: no row of its own any more: the menu shows it as the two rows «режим» + «масштаб» of the upscaler choice (E38),
    // this stored level (mode + scale in one number, unchanged) is what they edit; it counts only while E38 = MetalFX.
    Entry {
        token: "E34",
        name: ["MetalFX mode and scale", "MetalFX: режим и масштаб"],
        help: [
            "Apple's MetalFX scaler. The first four levels are TEMPORAL: the 3D scene is drawn jittered at a fraction of the window's size (or at full size for 'smoothing only', a temporal anti-aliaser) with motion vectors and depth, and MetalFX rebuilds the window-sized picture from several frames (sharper than a plain upscale, smooth edges; replaces MSAA, SMAA and 'render scale' while on; bloom, tone map and the like run at window size after it). The last three, 'simple', are the plain spatial scaler of one frame (no vectors; a little softer). The menu, the HUD and the mirror stay sharp. macOS only; where MetalFX is unavailable the row does nothing",
            "масштабатор Apple MetalFX. Первые четыре уровня ВРЕМЕННЫЕ: 3D-сцена рисуется со сдвигом субпикселя в доле размера окна (или в полном размере для «только сглаживания» — временной антиалиасинг) вместе с векторами движения и глубиной, и MetalFX собирает картинку размера окна из нескольких кадров (чётче простого растяжения, гладкие края; пока включён, заменяет MSAA, SMAA и «масштаб рендера»; свечение, тонемап и прочее работают после него в размере окна). Последние три, «простой», — обычный пространственный масштабатор одного кадра (без векторов; чуть мягче). Меню, приборы и зеркало остаются чёткими. Только macOS; где MetalFX недоступен, пункт ничего не делает",
        ],
        // 1..=4 temporal (`E34_TEMPORAL_LEVELS`), 5..=7 the spatial ones (the old 1..=3, renumbered by the settings file's version 3)
        levels: &[
            ["off", "выкл"],
            ["smoothing only (1:1)", "только сглаживание (1:1)"],
            ["quality (67 %)", "качество (67 %)"],
            ["balanced (59 %)", "баланс (59 %)"],
            ["performance (50 %)", "производительность (50 %)"],
            ["simple quality (67 %)", "простой: качество (67 %)"],
            ["simple balanced (59 %)", "простой: баланс (59 %)"],
            ["simple performance (50 %)", "простой: производительность (50 %)"],
        ],
        default_level: 0,
    },
    // ENHANCEMENT E33 - registered on every platform (indices must not depend on the platform); off macOS the
    // renderer reports no EDR, the row says «недоступно» and nothing happens
    Entry {
        token: "E33",
        name: ["HDR display (EDR)", "HDR-экран (EDR)"],
        help: [
            "extended-range output on a display that shows values above white (MacBook Pro XDR, Pro Display XDR): the picture keeps its normal brightness (white stays white) and only the highlights (the sun, lamps, sparks, bloom peaks, the glare on paint) go brighter than white with a soft shoulder. The HUD and the menus stay plain white. Needs 'HDR and tone mapping'. If the display cannot do it the row says 'unavailable' and does nothing",
            "расширенный диапазон на экране, который показывает ярче белого (MacBook Pro XDR, Pro Display XDR): картинка остаётся прежней яркости (белый остаётся белым), а ярче белого становятся только блики (солнце, фонари, искры, пики свечения, блики на краске) с мягким плечом. Индикаторы и меню остаются обычным белым. Нужен «HDR и тонемап». Если экран не умеет, строка показывает «недоступно» и ничего не делает",
        ],
        // the third caption is shown instead of the row while the display has no EDR; it is never a level
        levels: &[["off", "выкл"], ["on", "вкл"], ["unavailable", "недоступно"]],
        // off until judged on a real screen (the output is re-tagged: extended linear sRGB)
        default_level: 0,
    },
    // ENHANCEMENT E37: AMD FSR 3.1 temporal upscaling (every platform since ENH-UPSCALER; the menu shows its level as FSR's «масштаб»
    // row of the upscaler choice (E38), it counts only while E38 = FSR 3.1)
    Entry {
        token: "E37",
        name: ["FSR 3.1 scale", "FSR 3.1: масштаб"],
        help: [
            "AMD FidelityFX Super Resolution 3.1, a temporal upscaler: the 3D scene is drawn a little smaller with a sub-pixel jitter and rebuilt at the window's size from several frames (it replaces MSAA, SMAA and 'render scale' while on; the bloom, the exposure and the tone map then work at full size; the menu, the HUD and the mirror stay sharp). 'AA only' is native size with FSR's anti-aliasing; the others draw smaller and save the 3D work, at a small cost in fine detail. FSR itself is a set of compute passes at the window's size: it pays on a graphics card with power to spare, on a weak integrated GPU it costs more than it saves. Windows and Linux",
            "AMD FidelityFX Super Resolution 3.1, временный (temporal) апскейлер: 3D-сцена рисуется чуть меньше со сдвигом на доли пикселя и собирается до размера окна из нескольких кадров (заменяет MSAA, SMAA и «масштаб рендера», пока включён; свечение, экспозиция и тонемап работают уже в полном размере; меню, приборы и зеркало остаются чёткими). «Только сглаживание» — родной размер с антиалиасингом FSR; остальные уровни рисуют 3D меньше и экономят на нём, ценой небольшой потери мелких деталей. Сам FSR — это вычислительные проходы в размере окна: он окупается на видеокарте с запасом мощности, на слабой встроенной видеокарте стоит дороже, чем экономит. Windows и Linux",
        ],
        levels: &[["off", "выкл"], ["AA only (1:1)", "только сглаживание (1:1)"], ["quality (1.5x)", "качество (1.5x)"], ["balanced (1.7x)", "баланс (1.7x)"], ["performance (2x)", "производительность (2x)"]],
        default_level: 0,
    },
    // ENHANCEMENT ENH-UPSCALER (registered last): the one choice of the upscaler backend; E34 / E37 keep their stored levels and
    // are the settings of the chosen backend (the panel shows only those rows). Level 1 (MetalFX) exists on macOS only.
    Entry {
        token: "E38",
        name: ["upscaler", "апскейлер"],
        help: [
            "off, MetalFX (macOS) or AMD FSR 3.1: the 3D scene is drawn smaller and enlarged to the window, which saves GPU work; the settings of the chosen upscaler follow below. While one is on, 'render scale' has no effect. On a Mac FSR works but is slower than MetalFX (and than no upscaler) — MetalFX is the Mac's own",
            "выкл, MetalFX (macOS) или AMD FSR 3.1: 3D-сцена рисуется меньше и растягивается до размера окна, это экономит работу видеокарты; ниже идут настройки выбранного апскейлера. Пока он включен, «масштаб рендера» не действует. На Mac FSR работает, но медленнее MetalFX (и медленнее, чем без апскейлера) — родной для Mac вариант MetalFX",
        ],
        levels: &[["off", "выкл"], ["MetalFX", "MetalFX"], ["FSR 3.1", "FSR 3.1"]],
        default_level: 0,
    },
    // ENHANCEMENT ENH-UPSCALER: FSR's RCAS sharpening (the MetalFX scaler has none); a row of the FSR choice. 0.3 is what E37 had as a constant.
    Entry {
        token: "E39",
        name: ["FSR sharpness", "резкость FSR"],
        help: [
            "FSR 3.1's contrast-adaptive sharpening (RCAS) after the upscale: it counters the temporal filter's softness; too much rings on thin lines and foliage. off, or the strength 0.2 / 0.3 / 0.5 / 0.8",
            "контрастно-адаптивная резкость FSR 3.1 (RCAS) после апскейла: убирает мягкость временного фильтра; слишком сильная даёт ореолы на тонких линиях и листве. Выкл или сила 0.2 / 0.3 / 0.5 / 0.8",
        ],
        levels: &[["off", "выкл"], ["0.2", "0.2"], ["0.3", "0.3"], ["0.5", "0.5"], ["0.8", "0.8"]],
        default_level: 2,
    },
    // ENHANCEMENT E40: raindrops on the windscreen of the driver view (registered LAST, after E39)
    Entry {
        token: "E40",
        name: ["raindrops on the windscreen", "капли на лобовом стекле"],
        help: [
            "in the driver (cockpit) view, while it rains and the car is not under cover, refractive drops sit on the windscreen: they appear by the rain's strength and the speed, run down the glass when you are slow and are blown up and outward by the airflow at speed, merge into streaks and are wiped off every few seconds (the model's wipers are not animated, so the wipe is a soft sweep). The dashboard and the pillars stay clean; the original's own two drops stay too. Needs the HDR scene (E7); nothing in the chase and bumper views, the mirror or on a dry track",
            "в виде из кабины, пока идёт дождь и машина не под крышей, на лобовом стекле лежат преломляющие капли: они появляются по силе дождя и скорости, на малой скорости стекают вниз, на скорости воздух гонит их вверх и в стороны, они сливаются в струйки, а каждые несколько секунд стекло стирается (дворники модели не анимированы, поэтому стирание - мягкий проход). Панель и стойки остаются чистыми; две капли оригинала остаются тоже. Нужна HDR-сцена (E7); в видах сзади и с бампера, в зеркале и на сухой трассе ничего",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
    // ENHANCEMENT E41: the HD texture packs (registered LAST, after E40). Not a GPU pass: the loaders of the track, the sky and the car pick
    // the pack's images over the original's pixels when the level's pack is on disk; level 0 never looks for a file (master's path exactly).
    // Levels: 0 off, 1 on (the x4 pack); the row is grey without a pack beside the game (`Cond::NoHdPack`).
    Entry {
        token: "E41",
        name: ["enhanced textures", "улучшенные текстуры"],
        help: [
            "the HD pack, every texture enlarged x4 by a neural model (about 16x the original's video memory), for the track's surfaces, the horizon mountains and the clouds, and the cars' bodies, wheels and interiors, where the pack has them; the HUD, the menus, the fonts, the lens flares, the particles and everything else keep the original's. The pack is not part of the game: put a folder hd_textures beside it and restart",
            "HD-набор: все текстуры увеличены нейросетью в 4 раза (около 16 размеров оригинала по видеопамяти) для поверхностей трассы, гор на горизонте и облаков, кузова, колес и салона машин, где в наборе они есть; индикаторы, меню, шрифты, блики объектива, частицы и все остальное остаются оригинальными. Набор не входит в игру: положите папку hd_textures рядом с ней и перезапустите",
        ],
        levels: &[["off", "выкл"], ["on", "вкл"]],
        default_level: 1,
    },
    // ENHANCEMENT E42: rounder wheels, mirrors, lamps, exhausts, steering wheel (registered LAST, after E41). Not a GPU pass: the car
    // loader builds a render-only refined copy of the round parts after its measurements; level 0 never builds one (master's path).
    Entry {
        token: "E42",
        name: ["rounded details", "скругление деталей"],
        help: [
            "the round parts of the cars are refined when the car is loaded: the wheels' tyres and rims become true circles (32 segments, or 64 on the strong level, instead of the original's 16 or fewer, same texture mapping and radius), the side mirrors, lamp lenses, exhaust tips, the steering wheel and the police siren get smoothly curved surfaces by their own authored normals (hard edges stay sharp, outlines stay put). The body panels are untouched. Only near levels of detail; the physics, the damage and the replays do not change. Takes effect with the next car load (restart the race, re-enter the garage)",
            "круглые детали машин уточняются при загрузке машины: шины и диски колёс становятся настоящими окружностями (32 сегмента, на сильном уровне 64, вместо 16 и меньше у оригинала, та же текстурная раскладка и тот же радиус), боковые зеркала, линзы фар, выхлопные трубы, руль и мигалка полиции получают плавно изогнутые поверхности по собственным нормалям (резкие рёбра остаются резкими, контуры на месте). Панели кузова не затрагиваются. Только ближние уровни детализации; физика, повреждения и повторы не меняются. Действует со следующей загрузки машины (перезапустите заезд, зайдите в гараж заново)",
        ],
        levels: &[["off", "выкл"], ["soft", "мягко"], ["strong", "сильно"]],
        default_level: 1,
    },
    // ENHANCEMENT E43: the soft contact shadow under the cars (registered LAST, after E42). A render-only ambient-occlusion darkening of the
    // road under and round every near car, evaluated analytically in the track's fragment shader from the car's pose; where it draws, the
    // original's blob shadow gives way to it. Off = the original's blob (and E9's own rules) exactly.
    Entry {
        token: "E43",
        name: ["contact shadow", "контактная тень"],
        help: [
            "a soft dark contact shadow under and round every near car (strongest at the tyres and under the body, fading outward and with the car's height above the road), like ambient occlusion from the sky light. It stands in for the original's blob wherever the sun's shadow (E9) is not drawn: in tunnels and under bridges, at night, under an overcast sky, and fades smoothly over the sun's shadow at a tunnel mouth. Only the picture changes",
            "мягкая тёмная контактная тень под машиной и вокруг каждой ближней машины (сильнее всего у шин и под кузовом, плавно слабеет наружу и с высотой машины над дорогой), как затенение от неба. Заменяет пятно оригинала там, где нет тени от солнца (E9): в тоннелях и под мостами, ночью, под пасмурным небом, а у въезда в тоннель плавно сменяет тень от солнца. Меняется только картинка",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
    // ENHANCEMENT E44: the coastal lighthouse's beam drawn as a soft volumetric light (registered LAST, after E43). Needs E7 (the float scene):
    // with E7 off the original's translucent box stays.
    Entry {
        token: "E44",
        name: ["lighthouse beam", "луч маяка"],
        help: [
            "the lighthouse's beam on the coast track is drawn as soft light in the air instead of the original's flat translucent box with its straight edges and flat end: a Gaussian cone round the beam's own axis that turns with the lamp, bright near the lantern and fading smoothly with the distance, a little stronger at dusk and when it points at you, hidden behind hills and cars. The lantern itself and its glare are untouched. Needs the HDR scene (E7); without it the original's box is drawn. Only the picture changes",
            "луч маяка на трассе у моря рисуется мягким светом в воздухе вместо плоской полупрозрачной коробки оригинала с прямыми краями и плоским торцом: гауссов конус вокруг собственной оси луча, которая вращается вместе с лампой, яркий у фонаря и плавно слабеющий с расстоянием, чуть сильнее в сумерках и когда направлен на вас, скрывается за холмами и машинами. Сам фонарь и его блик не меняются. Нужен HDR-режим (E7); без него рисуется коробка оригинала. Меняется только картинка",
        ],
        levels: ON_OFF,
        default_level: 1,
    },
];

/// The [`Switch`] constants. `E7 = 6` declares `pub const E7: Switch = Switch(6)` and checks AT COMPILE TIME that `REGISTRY[6]` is
/// the entry with the token `E7`; with the tokens unique and one constant per registry row (both checked at compile time too) a
/// constant cannot point at another entry. Indices are stored positions: REGISTRY's order never changes, a new
/// entry is appended there and here.
macro_rules! switches {
    ($($(#[$doc:meta])* $name:ident = $index:literal,)*) => {
        $($(#[$doc])* pub const $name: Switch = Switch($index);)*
        const _: () = {
            $(assert!(str_eq(REGISTRY[$index].token, stringify!($name)), concat!("REGISTRY[", stringify!($index), "] is not ", stringify!($name)));)*
            assert!([$($index),*].len() == REGISTRY.len(), "a REGISTRY entry has no switch constant");
        };
    };
}

/// `a == b` in a const context.
const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// No two entries of `reg` share a token.
const fn tokens_unique(reg: &[Entry]) -> bool {
    let mut i = 0;
    while i < reg.len() {
        let mut j = i + 1;
        while j < reg.len() {
            if str_eq(reg[i].token, reg[j].token) {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}

const _: () = assert!(tokens_unique(REGISTRY), "two REGISTRY entries share a token");
const _: () = assert!(REGISTRY.len() <= MAX_SWITCHES, "REGISTRY has more entries than an Enhancements holds (MAX_SWITCHES)");

switches! {
    E1 = 0,
    E2 = 1,
    E3 = 2,
    E4 = 3,
    E5 = 4,
    E6 = 5,
    E7 = 6,
    E8 = 7,
    E9 = 8,
    /// ENHANCEMENT E12 soft particles
    E12 = 9,
    E10 = 10,
    E11 = 11,
    /// ENHANCEMENT E16 car light from the surroundings (SH probe of E8's faces)
    E16 = 12,
    /// ENHANCEMENT E15 headlight spots
    E15 = 13,
    E13 = 14,
    /// ENHANCEMENT E17 water
    E17 = 15,
    /// ENHANCEMENT E14 corner shading (AO)
    E14 = 16,
    /// ENHANCEMENT E18 particles in HDR
    E18 = 17,
    /// ENHANCEMENT E19 fog sun in-scatter + output dither
    E19 = 18,
    /// ENHANCEMENT E20 lightning lights the scene
    E20 = 19,
    /// ENHANCEMENT E21 eye adaptation
    E21 = 20,
    /// ENHANCEMENT E23 street lamps as lights
    E23 = 21,
    /// ENHANCEMENT E24 headlights light up in tunnels (render side only)
    E24 = 22,
    /// ENHANCEMENT E25 soft shadows on the sunless tracks
    E25 = 23,
    /// ENHANCEMENT E22 frame rate limit
    E22 = 24,
    /// ENHANCEMENT E27 motion blur
    E27 = 25,
    /// ENHANCEMENT E28 lens dirt
    E28 = 26,
    /// ENHANCEMENT E29 camera shake
    E29 = 27,
    /// ENHANCEMENT E26 wet road and rain
    E26 = 28,
    // index 29 was E30 (photo mode), removed 2026-10-27 (photo mode is always on now, PORT-DEVIATION D46 of master); the
    // later switches moved down by one. Nothing stores an index: `enhancements.json` and `NFS_ENH` name the tokens, which are
    // unchanged (an old file's "E30" key is ignored, an `NFS_ENH` "E30" is reported unknown)
    /// ENHANCEMENT E31 depth of field in replays
    E31 = 29,
    /// ENHANCEMENT E32 baked bounce light (global illumination)
    E32 = 30,
    /// ENHANCEMENT E35 SMAA 1x over the tone-mapped image
    E35 = 31,
    /// ENHANCEMENT E36 tunnel haze and visible headlight beams in fog / rain / tunnels
    E36 = 32,
    /// ENHANCEMENT E34 MetalFX upscaling (registered on every platform, after E36, so the other indices do not move; only macOS has the backend)
    E34 = 33,
    /// ENHANCEMENT E33 HDR display / EDR output (registered on every platform, after E34; the backend is macOS only)
    E33 = 34,
    /// ENHANCEMENT E37 FSR 3.1 temporal upscaling (registered last, on every platform; the backend is Windows / Linux, macOS only with `NFS_FSR_FORCE`)
    E37 = 35,
    /// ENHANCEMENT ENH-UPSCALER: the upscaler choice (0 off, 1 MetalFX, 2 FSR 3.1; registered after E37, on every platform, level 1 is macOS only)
    E38 = 36,
    /// ENHANCEMENT ENH-UPSCALER: FSR's RCAS sharpness (0 off, then 0.2 / 0.3 / 0.5 / 0.8; used only while E38 = FSR)
    E39 = 37,
    /// ENHANCEMENT E40 raindrops on the windscreen of the driver view (registered last, after E39)
    E40 = 38,
    /// ENHANCEMENT E41 the HD texture pack: 0 off, 1 on (registered last, after E40; read by the asset loaders, no GPU pass)
    E41 = 39,
    /// ENHANCEMENT E42 rounded details of the cars: 0 off, 1 soft, 2 strong (registered last, after E41; read by the car loader, no GPU pass)
    E42 = 40,
    /// ENHANCEMENT E43 the soft contact shadow under the cars: 0 off, 1 on (registered last, after E42)
    E43 = 41,
    /// ENHANCEMENT E44 the lighthouse beam as soft volumetric light: 0 off, 1 on (registered last, after E43; needs E7)
    E44 = 42,
}

/// ENHANCEMENT E34: its levels 1..=this are the temporal ones (MetalFX temporal scaler); the levels above are the spatial ('simple') ones
pub const E34_TEMPORAL_LEVELS: u8 = 4;

// ---- ENHANCEMENT ENH-UPSCALER: the upscaler choice and the settings rows of the chosen backend ----

/// E38's levels.
pub const UP_OFF: u8 = 0;
pub const UP_METALFX: u8 = 1;
pub const UP_FSR: u8 = 2;

/// The E38 levels this platform offers, in the combo's order: macOS off / MetalFX / FSR 3.1, elsewhere off / FSR 3.1.
pub fn upscaler_choices() -> &'static [u8] {
    if cfg!(target_os = "macos") { &[UP_OFF, UP_METALFX, UP_FSR] } else { &[UP_OFF, UP_FSR] }
}

/// MetalFX's mode, the first of its two rows: E34 levels 1..=4 are the temporal scaler, 5..=7 the spatial («простой») one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfxMode {
    Temporal,
    Simple,
}

/// E34's stored level -> the mode row (level 0, "off", counts as the default temporal quality level 2, see [`Enhancements::effective`]).
pub fn mfx_mode(level: u8) -> MfxMode {
    if level > E34_TEMPORAL_LEVELS { MfxMode::Simple } else { MfxMode::Temporal }
}

/// E34's stored level -> the scale row's index: temporal 1:1 / 67 / 59 / 50 % = 0..=3 (levels 1..=4), simple 67 / 59 / 50 % = 0..=2
/// (levels 5..=7). Level 0 reads as the default (67 %, temporal).
pub fn mfx_scale_index(level: u8) -> usize {
    match level {
        0 => 1,
        1..=4 => level as usize - 1,
        _ => (level - E34_TEMPORAL_LEVELS - 1) as usize,
    }
}

/// The number of choices of the scale row of a mode: temporal 4 (1:1 too), simple 3.
pub fn mfx_scale_count(mode: MfxMode) -> usize {
    match mode {
        MfxMode::Temporal => 4,
        MfxMode::Simple => 3,
    }
}

/// The E34 level of a (mode, scale index); an index past the mode's last is clamped; the simple mode has no 1:1, so temporal's
/// 1:1 (index 0) becomes simple's 67 % (index 0 there is already 67 %: the two scale lists are offset by the 1:1 entry).
pub fn mfx_level(mode: MfxMode, scale: usize) -> u8 {
    match mode {
        MfxMode::Temporal => 1 + scale.min(3) as u8,
        MfxMode::Simple => E34_TEMPORAL_LEVELS + 1 + scale.min(2) as u8,
    }
}

/// Switching the mode keeps the scale where it exists: temporal 67 / 59 / 50 % (levels 2..=4) <-> simple 67 / 59 / 50 % (5..=7); the
/// temporal 1:1 (level 1) goes to simple 67 %.
pub fn mfx_switch_mode(level: u8, to: MfxMode) -> u8 {
    let l = if level == 0 { 2 } else { level };
    match (mfx_mode(l), to) {
        (MfxMode::Temporal, MfxMode::Simple) => E34_TEMPORAL_LEVELS + l.max(2) - 1,
        (MfxMode::Simple, MfxMode::Temporal) => l - E34_TEMPORAL_LEVELS + 1,
        _ => l,
    }
}

/// FSR's scale row: E37 levels 1..=4 = 1:1 / 1.5x / 1.7x / 2x = indices 0..=3 (level 0 reads as 1.5x, the default).
pub fn fsr_scale_index(level: u8) -> usize {
    if level == 0 { 1 } else { level as usize - 1 }
}

/// The RCAS strength of E39's level; `None` = sharpening off.
pub fn fsr_sharpness(level: u8) -> Option<f32> {
    match level {
        0 => None,
        1 => Some(0.2),
        2 => Some(0.3),
        3 => Some(0.5),
        _ => Some(0.8),
    }
}

// ---- the level -> value maps of the other entries, pure, beside the table. The renderer's `enh_*` and
// the app's `enh_*` keep the gates (is the switch on, does the device / the scene allow it) and call these. The values are the
// authored ones, unchanged. ----

/// ENHANCEMENT E1: the sample count a level asks for (the renderer takes the highest the adapter has that is `<=` it); 1 at
/// level 0.
pub fn msaa_samples_wanted(level: u8) -> u32 {
    match level {
        0 => 1,
        1 => 2,
        2 => 4,
        _ => 8,
    }
}

/// ENHANCEMENT E3: the render scale of a level (level 0 = 1.0, the original 1:1).
pub fn render_scale(level: u8) -> f32 {
    match level {
        0 => 1.0,
        1 => 0.5,
        2 => 0.75,
        3 => 1.25,
        4 => 1.5,
        _ => 2.0,
    }
}

/// ENHANCEMENT E4: degrees added to the original's horizontal view angle (0 at level 0, then 5 per level).
pub fn fov_bonus_deg(level: u8) -> f32 {
    level as f32 * 5.0
}

/// ENHANCEMENT E5: the draw-distance factor of the main view (1.0, the original, at level 0; +25 / +50 / +100 %).
pub fn draw_reach(level: u8) -> f32 {
    match level {
        1 => 1.25,
        2 => 1.5,
        3 => 2.0,
        _ => 1.0,
    }
}

/// ENHANCEMENT E7: the tone-map mode of a level that is on: 0 "as the original" (level 1), 1 ACES (level 2).
pub fn tonemap_mode(level: u8) -> u32 {
    if level >= 2 { 1 } else { 0 }
}

/// ENHANCEMENT E13: the light shafts' strength factor at level 1 (subtle) and level 2 (medium).
const SHAFT_STRENGTH_SUBTLE: f32 = 0.45;
const SHAFT_STRENGTH_MEDIUM: f32 = 0.8;

/// ENHANCEMENT E13: the factor on the scene's own shaft strength of a level that is on.
pub fn shaft_strength(level: u8) -> f32 {
    if level >= 2 { SHAFT_STRENGTH_MEDIUM } else { SHAFT_STRENGTH_SUBTLE }
}

/// ENHANCEMENT E26: the wet road's reflection gain (`globals.rain.x`) at level 1 and at level 2 and up.
const RAIN_GAIN: f32 = 1.0;
const RAIN_GAIN_L2: f32 = 1.5;

/// ENHANCEMENT E26: the wet road's reflection gain of a level that is on.
pub fn rain_gain(level: u8) -> f32 {
    if level >= 2 { RAIN_GAIN_L2 } else { RAIN_GAIN }
}

/// ENHANCEMENT E23: a street lamp's pool scale (on its radius) and light gain of a level that is on: soft (level 1) a smaller,
/// dimmer pool, normal the authored default.
pub fn street_lamp_pool(level: u8) -> (f32, f32) {
    if level == 1 { (0.75, 0.7) } else { (1.0, 1.0) }
}

/// ENH-LITE: the «лёгкий» (light) preset — the levels a weak (integrated) GPU runs the enhanced game on.
/// A token listed gets that level, **every other entry is 0 (off)** (so an enhancement registered later is
/// off in the light preset until someone measures it and lists it here). Chosen from the ablation on a
/// real weak GPU (Intel UHD Ice Lake GT1, Vulkan, 1920x1080, alps with 3 opponents): the entries that keep
/// the most visible look for the least frame time; the table and the measurements are in
/// `RE/notes/ENHANCEMENTS.md` "Light preset (ENH-LITE)". Render-only like every enhancement.
pub const LITE_PRESET: &[(Switch, u8)] = &[
    (E1, 1),  // MSAA 2x: +3 ms — 2026-10-04: smooth edges AND the reflections, both
    (E2, 3),  // anisotropic 16x: +0.4 ms, the sharpest road at no real cost
    (E5, 2),  // draw distance +50 %: +0.1 ms
    (E7, 1),  // HDR scene "as the original": +1.5 ms, the base the others need
    (E8, 2),  // body clear coat, normal: a few tenths of a ms with E16 (the 256 px cube)
    (E12, 1), // soft particles: ~0.2 ms
    (E16, 1), // car ambient probe (needs E8)
    (E17, 1), // water: 0 where there is none, ~0 where there is
    (E18, 1), // particles in HDR
    (E19, 1), // fog sun glow + dither: +0.3 ms
    (E20, 1), // lightning (two tracks)
    (E21, 1), // eye adaptation: +0.3 ms
    (E23, 1), // street lamps, soft: 0 by day, +5.6 ms on a night street (the one costly entry kept: the night look)
    (E24, 1), // headlights in tunnels
    (E26, 1), // wet road and rain
    (E32, 2), // baked bounce light, medium as the full defaults (baked once per track: no frame cost)
    (E42, 1), // rounded details, soft: load-time CPU (~10 ms a car) and a few thousand triangles, no render pass
    (E43, 1), // contact shadow: stands in for the blob (the light preset has no E9), analytic, a few tenths of a ms
    (E44, 1), // lighthouse beam: one analytic full-screen pass on coastal only, a few tenths of a ms there
    // no SMAA (E35, +5.6 ms on this GPU)
];

/// ENHANCEMENT ENH-UPSCALER: when a setting cannot act. Every row of [`DEPENDS`] says "entry `entry` has no effect while `cond`
/// holds"; the panel shows such a row inactive with the reason as its hint (its stored level is kept: switching the cause back
/// restores it), and [`Enhancements::resolved`] applies the `Needs` rows to a set (data only; the unit test
/// `resolved_zeroes_the_dependents` checks that it and [`Enhancements::blocked`] agree). Derived from the code's `is_on` / `level`
/// gates in `gpu.rs` and `gpu/csm.rs`, 2026-10-04.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    /// the entry with this token is off (level 0)
    Needs(&'static str),
    /// an upscaler is chosen (E38): it sets the internal size itself, so E3's render scale is not used
    UpscalerOn,
    /// a TEMPORAL upscaler is active (FSR 3.1, or MetalFX's temporal mode): the main view is drawn at 1 sample with no
    /// post-process anti-aliasing (E1 MSAA and E35 SMAA are replaced by the upscaler's own temporal filter)
    TemporalUpscaler,
    /// no HD texture pack was found beside the game ([`set_hd_textures_available`])
    NoHdPack,
}

/// Whether this run has an HD texture pack, set once at start-up: E41's row is grey without one.
static HD_TEXTURES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_hd_textures_available(yes: bool) {
    HD_TEXTURES.store(yes, std::sync::atomic::Ordering::Relaxed);
}

pub fn hd_textures_available() -> bool {
    HD_TEXTURES.load(std::sync::atomic::Ordering::Relaxed)
}

/// One "has no effect while" row, see [`Cond`].
#[derive(Debug, Clone, Copy)]
pub struct Dependency {
    pub entry: &'static str,
    pub cond: Cond,
}

/// Every dependency / override of the enhancements (the panel's inactive rows; the renderer documents them here too).
/// Only RULES THE CODE ENFORCES are listed: a "needs" that the help texts mention but no gate checks is not here (E36's beams and
/// E15 spotlights, E24 and E15, E19's fog glow which does not need HDR, E22's levels
/// 5 / 6 with the frame output (a fallback, not a block), E17 / E8 (only at night, partial)).
pub const DEPENDS: &[Dependency] = &[
    Dependency { entry: "E41", cond: Cond::NoHdPack },
    // the float scene (E7) is what the post effects work on: with E7 off `enh_hdr` gives None and every `hdr` / `cur_hdr` gate fails
    Dependency { entry: "E10", cond: Cond::Needs("E7") },
    Dependency { entry: "E11", cond: Cond::Needs("E7") },
    Dependency { entry: "E13", cond: Cond::Needs("E7") },
    Dependency { entry: "E18", cond: Cond::Needs("E7") },
    Dependency { entry: "E21", cond: Cond::Needs("E7") },
    Dependency { entry: "E27", cond: Cond::Needs("E7") },
    Dependency { entry: "E31", cond: Cond::Needs("E7") },
    Dependency { entry: "E33", cond: Cond::Needs("E7") },
    Dependency { entry: "E40", cond: Cond::Needs("E7") },
    Dependency { entry: "E36", cond: Cond::Needs("E7") },
    Dependency { entry: "E44", cond: Cond::Needs("E7") },
    // the lens dirt is added by the bloom pass (inside `if bloom_level > 0`), so it needs E10 and, through it, E7
    Dependency { entry: "E28", cond: Cond::Needs("E7") },
    Dependency { entry: "E28", cond: Cond::Needs("E10") },
    // the car-ambient probe samples E8's environment faces
    Dependency { entry: "E16", cond: Cond::Needs("E8") },
    // the overcast shadows are a second light of E9's cascades
    Dependency { entry: "E25", cond: Cond::Needs("E9") },
    // an upscaler draws the scene at its own internal size: E3's scale is not used; a temporal one also draws at 1 sample with
    // no SMAA pass (the spatial MetalFX mode keeps both)
    Dependency { entry: "E3", cond: Cond::UpscalerOn },
    Dependency { entry: "E1", cond: Cond::TemporalUpscaler },
    Dependency { entry: "E35", cond: Cond::TemporalUpscaler },
];

/// Why a row is inactive: the [`DEPENDS`] row, and for the upscaler conditions which backend (the E38 level, to name it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blocked {
    pub dep: usize,
    pub by: u8,
}

impl Enhancements {
    /// ENHANCEMENT ENH-UPSCALER: the set the renderer works with. E34 (MetalFX's mode + scale) counts only while E38 = MetalFX (macOS),
    /// E37 (FSR's scale) only while E38 = FSR 3.1; the other is 0. A chosen backend whose level is still 0 (`NFS_ENH=E38=2`, a first
    /// choice) takes the default: MetalFX temporal 67 % (E34 = 2), FSR 1.5x (E37 = 2). The stored set keeps every value (a hidden row's
    /// level survives); only this view zeroes them. With `E38` = 0 and E34 / E37 > 0 in a hand-made set nothing is on.
    pub fn effective(&self) -> Self {
        let mut e = *self;
        let up = match e.level(E38) {
            UP_METALFX if !cfg!(target_os = "macos") => UP_OFF,
            u => u,
        };
        if up == UP_METALFX {
            if e.level(E34) == 0 {
                e.set_level(E34, 2);
            }
        } else {
            e.set_level(E34, 0);
        }
        if up == UP_FSR {
            if e.level(E37) == 0 {
                e.set_level(E37, 2);
            }
        } else {
            e.set_level(E37, 0);
        }
        e
    }

    /// The old files / `NFS_ENH` lists have no E38: a set with E34 > 0 (macOS) means MetalFX, else with E37 > 0 means FSR 3.1 (both: the
    /// platform's own, MetalFX on macOS). FSR's sharpness (E39), new, then takes its default (E37 always had 0.3) unless `e39_named`.
    pub fn infer_upscaler(&mut self, e39_named: bool) {
        let up = if self.level(E34) > 0 && cfg!(target_os = "macos") {
            UP_METALFX
        } else if self.level(E37) > 0 {
            UP_FSR
        } else {
            return;
        };
        self.set_level(E38, up);
        if up == UP_FSR && !e39_named && self.level(E39) == 0 {
            self.set_level(E39, REGISTRY[E39.0 as usize].default_level);
        }
    }

    /// Whether a TEMPORAL upscaler is active (the effective set: FSR 3.1, or MetalFX's temporal mode).
    pub fn temporal_upscaler(&self) -> bool {
        let e = self.effective();
        e.level(E37) > 0 || (1..=E34_TEMPORAL_LEVELS).contains(&e.level(E34))
    }

    /// Why entry `token` has no effect now, `None` when it acts (or nothing is known to block it). The first unmet row of [`DEPENDS`].
    /// The level a row shows: its own, but "off" for the textures with no pack (the stored level is kept for when one appears).
    pub fn shown_level(&self, sw: Switch) -> u8 {
        if sw == E41 && !hd_textures_available() { 0 } else { self.level(sw) }
    }

    pub fn blocked(&self, token: &str) -> Option<Blocked> {
        let eff = self.effective();
        DEPENDS.iter().enumerate().filter(|(_, d)| d.entry == token).find_map(|(i, d)| {
            let hit = match d.cond {
                Cond::Needs(t) => REGISTRY.iter().position(|e| e.token == t).is_some_and(|k| self.levels[k] == 0),
                Cond::UpscalerOn => eff.level(E34) > 0 || eff.level(E37) > 0,
                Cond::TemporalUpscaler => self.temporal_upscaler(),
                Cond::NoHdPack => !hd_textures_available(),
            };
            hit.then(|| Blocked { dep: i, by: if matches!(d.cond, Cond::Needs(_)) { 0 } else { eff.level(E38).min(UP_FSR) } })
        })
    }

    /// The [`effective`](Self::effective) set with every `Needs` row of [`DEPENDS`] applied: an entry whose prerequisite reads 0
    /// reads 0 too, through chains (E28 -> E10 -> E7). Pure data, the registry's own statement of what can act; the renderer keeps
    /// its own gates (this does not replace them). The upscaler rows ([`Cond::UpscalerOn`] / [`Cond::TemporalUpscaler`]) are not
    /// levels and are not applied here.
    pub fn resolved(&self) -> Self {
        let mut e = self.effective();
        let index = |t: &str| REGISTRY.iter().position(|r| r.token == t);
        loop {
            let mut changed = false;
            for d in DEPENDS {
                let Cond::Needs(t) = d.cond else { continue };
                if let (Some(k), Some(i)) = (index(t), index(d.entry))
                    && e.levels[k] == 0
                    && e.levels[i] != 0
                {
                    e.levels[i] = 0;
                    changed = true;
                }
            }
            if !changed {
                return e;
            }
        }
    }
}

/// How strong the GPU is, for the first-run choice of the default set (ENH-LITE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuClass {
    /// a discrete GPU, Apple Silicon, anything not known to be weak: the full defaults
    Strong,
    /// an integrated GPU (not Apple Silicon) or a software rasteriser: the light preset
    Weak,
}

/// The class of an adapter: a CPU / software adapter or an integrated GPU is [`GpuClass::Weak`] —
/// except Apple Silicon (a Metal adapter named "Apple ...": integrated, but as strong as a mid-range
/// discrete card, the full defaults run it at 60 fps), and an Intel Mac's integrated GPU is weak.
pub fn classify_gpu(device_type: wgpu::DeviceType, backend: wgpu::Backend, name: &str) -> GpuClass {
    let apple_silicon = backend == wgpu::Backend::Metal && name.trim_start().to_ascii_lowercase().starts_with("apple");
    match device_type {
        wgpu::DeviceType::Cpu => GpuClass::Weak,
        wgpu::DeviceType::IntegratedGpu if !apple_silicon => GpuClass::Weak,
        _ => GpuClass::Strong,
    }
}

/// The most switches an [`Enhancements`] holds.
pub const MAX_SWITCHES: usize = 64;
/// The most levels an [`Entry`] has (the menu's string ids reserve this many per row).
pub const MAX_LEVELS: usize = 8;

/// One stored value of a switch: a level, or the old boolean form of the first file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    Level(u8),
    /// `true` = the entry's default level, `false` = off
    Bool(bool),
}

/// The level of every switch (0 = off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enhancements {
    levels: [u8; MAX_SWITCHES],
}

impl Default for Enhancements {
    fn default() -> Self {
        Self::ORIGINAL
    }
}

impl Enhancements {
    /// Every switch off: the original, `master` byte for byte.
    pub const ORIGINAL: Self = Self { levels: [0; MAX_SWITCHES] };

    /// Every switch at its default level (the shipped default).
    pub fn all() -> Self {
        Self::all_in(REGISTRY)
    }

    /// Whether the switch is on (its level is above 0).
    pub fn is_on(&self, s: Switch) -> bool {
        self.level(s) > 0
    }

    /// The switch's level; 0 = off, and always 0 for an index out of range.
    pub fn level(&self, s: Switch) -> u8 {
        self.levels.get(s.0 as usize).copied().unwrap_or(0)
    }

    /// Set a level (an index out of range is ignored; the caller keeps it below its entry's count).
    pub fn set_level(&mut self, s: Switch, level: u8) {
        if let Some(l) = self.levels.get_mut(s.0 as usize) {
            *l = level;
        }
    }

    /// Whether any switch is on — the guard the GPU side may test first.
    pub fn any_on(&self) -> bool {
        self.levels.iter().any(|&l| l > 0)
    }

    /// The raw levels (tests, logs).
    pub fn levels(&self) -> &[u8; MAX_SWITCHES] {
        &self.levels
    }

    /// `NFS_ENH`: `off` (the original), `all` (every default), or a list `E1,E3=0,E4=2` — a bare
    /// token is that switch at its default level, `token=n` an explicit level, the others off.
    /// `None` for an empty value. Case-insensitive; an unknown token or a level out of range in a
    /// list is skipped and pushed to `unknown`.
    pub fn parse_override(value: &str, unknown: &mut Vec<String>) -> Option<Self> {
        Self::parse_override_in(REGISTRY, value, unknown)
    }

    /// The switches that are on, `E1,E3=1`: a bare token at its default level; `all` / `off` when
    /// every one is at its default / none is on. Parses back to the same set.
    pub fn describe(&self) -> String {
        self.describe_in(REGISTRY)
    }

    /// The light preset ([`LITE_PRESET`]): the listed entries at their level, the rest off.
    pub fn lite() -> Self {
        Self::lite_in(REGISTRY, LITE_PRESET.iter().map(|(s, l)| (s.token(), *l)))
    }

    /// A preset table of `(token, level)` over a registry: the listed tokens at their level (clamped), the rest off, an unknown or
    /// unavailable token skipped.
    pub fn lite_in<'a>(reg: &[Entry], table: impl IntoIterator<Item = (&'a str, u8)>) -> Self {
        let mut out = Self::ORIGINAL;
        for (tok, lvl) in table {
            if let Some(i) = reg.iter().position(|e| e.token == tok).filter(|i| *i < MAX_SWITCHES && reg[*i].available()) {
                out.levels[i] = lvl.min(reg[i].levels.len().saturating_sub(1) as u8);
            }
        }
        out
    }

    // The same operations over any registry — what the unit tests drive.

    pub fn all_in(reg: &[Entry]) -> Self {
        let mut out = Self::ORIGINAL;
        for (l, e) in out.levels.iter_mut().zip(reg) {
            *l = if e.available() { e.default_level } else { 0 };
        }
        out
    }

    pub fn parse_override_in(reg: &[Entry], value: &str, unknown: &mut Vec<String>) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("none") {
            return Some(Self::ORIGINAL);
        }
        if v.eq_ignore_ascii_case("all") || v.eq_ignore_ascii_case("on") {
            return Some(Self::all_in(reg));
        }
        if v.eq_ignore_ascii_case("lite") || v.eq_ignore_ascii_case("light") {
            return Some(Self::lite_for(reg));
        }
        if v.is_empty() {
            return None;
        }
        let mut out = Self::ORIGINAL;
        // ENHANCEMENT ENH-UPSCALER: which of E38 / E39 the list names itself (the others are inferred from E34 / E37, see below)
        let (mut named_e38, mut named_e39) = (false, false);
        for tok in v.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let key = tok.split_once('=').map_or(tok, |(n, _)| n.trim());
            named_e38 |= key.eq_ignore_ascii_case("E38");
            named_e39 |= key.eq_ignore_ascii_case("E39");
            // `all,E16=2`: every default, then the listed overrides (a tooling convenience; `all` alone is as before)
            if tok.eq_ignore_ascii_case("all") {
                out = Self::all_in(reg);
                continue;
            }
            // `lite,E14=1`: the light preset, then the overrides
            if tok.eq_ignore_ascii_case("lite") || tok.eq_ignore_ascii_case("light") {
                out = Self::lite_for(reg);
                continue;
            }
            let (name, lvl) = match tok.split_once('=') {
                Some((n, l)) => (n.trim(), Some(l.trim())),
                None => (tok, None),
            };
            let Some(i) = reg.iter().position(|e| e.token.eq_ignore_ascii_case(name)).filter(|i| *i < MAX_SWITCHES && reg[*i].available()) else {
                unknown.push(tok.to_string());
                continue;
            };
            let level = match lvl {
                None => Some(reg[i].default_level),
                Some(l) => l.parse::<u8>().ok().filter(|l| (*l as usize) < reg[i].levels.len()),
            };
            match level {
                Some(l) => out.levels[i] = l,
                None => unknown.push(tok.to_string()),
            }
        }
        // ENHANCEMENT ENH-UPSCALER: the tokens of the old two-row menu keep working: `E34=2` / `E37=3` without an `E38` choose that backend
        if !named_e38 && std::ptr::eq(reg, REGISTRY) {
            out.infer_upscaler(named_e39);
        }
        Some(out)
    }

    /// The light preset of a registry: [`LITE_PRESET`] for the real one, an empty table for any other (the tests').
    fn lite_for(reg: &[Entry]) -> Self {
        if std::ptr::eq(reg, REGISTRY) { Self::lite() } else { Self::lite_in(reg, []) }
    }

    pub fn describe_in(&self, reg: &[Entry]) -> String {
        if !reg.is_empty() && *self == Self::all_in(reg) {
            return "all".into();
        }
        if std::ptr::eq(reg, REGISTRY) && *self == Self::lite() {
            return "lite".into();
        }
        let on: Vec<String> = reg
            .iter()
            .enumerate()
            .filter(|(i, _)| *i < MAX_SWITCHES && self.levels[*i] > 0)
            .map(|(i, e)| if self.levels[i] == e.default_level { e.token.to_string() } else { format!("{}={}", e.token, self.levels[i]) })
            .collect();
        if on.is_empty() { "off".into() } else { on.join(",") }
    }

    /// `(token, level)` pairs for a registry — the storage form.
    pub fn pairs_in(&self, reg: &[Entry]) -> Vec<(&'static str, u8)> {
        reg.iter().enumerate().filter(|(i, _)| *i < MAX_SWITCHES).map(|(i, e)| (e.token, self.levels[i])).collect()
    }

    /// The set a stored list gives; a switch the list does not name keeps its default level, so a
    /// newly registered enhancement is on (at its default) for a player with an older file. A stored
    /// level beyond the entry's choices is clamped to the highest one; an old boolean is `true` =
    /// the default level, `false` = off.
    pub fn from_stored_in<'a>(reg: &[Entry], pairs: impl IntoIterator<Item = (&'a str, Stored)>) -> Self {
        let mut out = Self::all_in(reg);
        for (tok, v) in pairs {
            if let Some(i) = reg.iter().position(|e| e.token == tok).filter(|i| *i < MAX_SWITCHES && reg[*i].available()) {
                out.levels[i] = match v {
                    Stored::Bool(true) => reg[i].default_level,
                    Stored::Bool(false) => 0,
                    Stored::Level(l) => l.min(reg[i].levels.len().saturating_sub(1) as u8),
                };
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn e(token: &'static str, levels: &'static [[&'static str; 2]], default_level: u8) -> Entry {
        Entry { token, name: ["n", "н"], help: ["h", "п"], levels, default_level }
    }
    const L4: &[[&str; 2]] = &[["off", "в"], ["a", "а"], ["b", "б"], ["c", "в"]];
    const TEST: &[Entry] = &[e("E1", ON_OFF, 1), e("E2", L4, 2), e("E3", L4, 3)];

    fn lv(s: &Enhancements) -> (u8, u8, u8) {
        (s.level(Switch(0)), s.level(Switch(1)), s.level(Switch(2)))
    }

    #[test]
    fn original_has_nothing_on_and_all_is_every_default() {
        let o = Enhancements::ORIGINAL;
        assert!(!o.any_on());
        let a = Enhancements::all_in(TEST);
        assert_eq!(lv(&a), (1, 2, 3));
        assert!((0..3).all(|i| a.is_on(Switch(i))));
        assert!(!a.is_on(Switch(3)) && a.level(Switch(200)) == 0);
        assert_eq!(a.describe_in(TEST), "all");
        assert_eq!(o.describe_in(TEST), "off");
        assert_eq!(Enhancements::default(), o);
    }

    #[test]
    fn set_level_and_is_on() {
        let mut s = Enhancements::ORIGINAL;
        s.set_level(Switch(1), 3);
        assert!(s.is_on(Switch(1)) && s.any_on() && s.level(Switch(1)) == 3);
        s.set_level(Switch(1), 0);
        assert!(!s.is_on(Switch(1)) && !s.any_on() && s == Enhancements::ORIGINAL);
        s.set_level(Switch(99), 1); // out of range: ignored
        assert_eq!(s, Enhancements::ORIGINAL);
    }

    #[test]
    fn the_empty_registry_has_all_equal_original() {
        assert_eq!(Enhancements::all_in(&[]), Enhancements::ORIGINAL);
        assert_eq!(Enhancements::all_in(&[]).describe_in(&[]), "off");
    }

    #[test]
    fn override_forms() {
        let mut unk = Vec::new();
        assert_eq!(Enhancements::parse_override_in(TEST, "off", &mut unk), Some(Enhancements::ORIGINAL));
        assert_eq!(Enhancements::parse_override_in(TEST, "ALL", &mut unk), Some(Enhancements::all_in(TEST)));
        // a bare token = its default level, the rest off
        let l = Enhancements::parse_override_in(TEST, "E1, e3,E9", &mut unk).unwrap();
        assert_eq!(lv(&l), (1, 0, 3));
        assert_eq!(unk, vec!["E9".to_string()]);
        assert_eq!(l.describe_in(TEST), "E1,E3");
        // explicit levels, 0 = off, out of range / not a number skipped
        unk.clear();
        let x = Enhancements::parse_override_in(TEST, "E1=0,E2=1,e3=2, E2=9, E1=x", &mut unk).unwrap();
        assert_eq!(lv(&x), (0, 1, 2));
        assert_eq!(unk, vec!["E2=9".to_string(), "E1=x".to_string()]);
        assert_eq!(x.describe_in(TEST), "E2=1,E3=2");
        assert_eq!(Enhancements::parse_override_in(TEST, "", &mut unk), None);
    }

    #[test]
    fn lite_preset_lists_the_given_levels_and_nothing_else() {
        // ENH-LITE: a listed token at its level (clamped), every other entry off, an unknown token skipped
        let l = Enhancements::lite_in(TEST, [("E2", 1), ("E3", 9), ("Enope", 1)]);
        assert_eq!(lv(&l), (0, 1, 3));
        assert_eq!(Enhancements::lite_in(TEST, []), Enhancements::ORIGINAL);
    }

    #[test]
    fn the_real_lite_preset_is_valid_and_parses() {
        for (sw, lvl) in LITE_PRESET {
            let e = &REGISTRY[sw.0 as usize];
            assert!((*lvl as usize) < e.levels.len() && *lvl > 0, "{}", e.token);
        }
        let lite = Enhancements::lite();
        assert!(lite.any_on() && lite != Enhancements::all());
        // no entry above its default level in the light preset (light = less than the defaults)
        for (i, e) in REGISTRY.iter().enumerate() {
            assert!(lite.levels()[i] <= e.default_level.max(1), "{}", e.token);
        }
        let mut unk = Vec::new();
        assert_eq!(Enhancements::parse_override("lite", &mut unk), Some(lite));
        assert_eq!(Enhancements::parse_override("LIGHT", &mut unk), Some(lite));
        assert_eq!(Enhancements::lite().describe(), "lite");
        // `lite,E14=1`: the preset, then the override
        let x = Enhancements::parse_override("lite,E14=1", &mut unk).unwrap();
        assert_eq!(x.level(E14), 1);
        assert_eq!(x.level(E1), lite.level(E1));
        assert!(unk.is_empty());
        // the light preset sets nothing on a platform without the entry's backend
        assert_eq!(lite.level(E33) + lite.level(E34), 0);
    }

    #[test]
    fn gpu_class_of_an_adapter() {
        use wgpu::{Backend::*, DeviceType::*};
        assert_eq!(classify_gpu(DiscreteGpu, Vulkan, "NVIDIA GeForce RTX 4060"), GpuClass::Strong);
        assert_eq!(classify_gpu(IntegratedGpu, Vulkan, "Intel(R) UHD Graphics (ICL GT1)"), GpuClass::Weak);
        assert_eq!(classify_gpu(IntegratedGpu, Dx12, "AMD Radeon(TM) Graphics"), GpuClass::Weak);
        assert_eq!(classify_gpu(Cpu, Vulkan, "llvmpipe (LLVM 15.0.6, 256 bits)"), GpuClass::Weak);
        // Apple Silicon: integrated but strong; an Intel Mac's integrated GPU is not
        assert_eq!(classify_gpu(IntegratedGpu, Metal, "Apple M1 Pro"), GpuClass::Strong);
        assert_eq!(classify_gpu(IntegratedGpu, Metal, "Intel(R) Iris(TM) Plus Graphics"), GpuClass::Weak);
        assert_eq!(classify_gpu(Other, Gl, "unknown"), GpuClass::Strong);
    }

    #[test]
    fn describe_parses_back() {
        let mut unk = Vec::new();
        for s in ["off", "all", "E1", "E2=1,E3", "E1,E2=3,E3=1"] {
            let e = Enhancements::parse_override_in(TEST, s, &mut unk).unwrap();
            assert_eq!(Enhancements::parse_override_in(TEST, &e.describe_in(TEST), &mut unk), Some(e), "{s}");
        }
        assert!(unk.is_empty());
    }

    #[test]
    fn stored_round_trip_and_defaults() {
        let mut s = Enhancements::all_in(TEST);
        s.set_level(Switch(1), 0);
        s.set_level(Switch(2), 1);
        let pairs = s.pairs_in(TEST);
        assert_eq!(pairs, vec![("E1", 1), ("E2", 0), ("E3", 1)]);
        assert_eq!(Enhancements::from_stored_in(TEST, pairs.into_iter().map(|(k, l)| (k, Stored::Level(l)))), s);
        // an older file that knows only E1 (off): E2 and E3, new since, are at their defaults
        let old = Enhancements::from_stored_in(TEST, [("E1", Stored::Bool(false)), ("Eold", Stored::Bool(false))]);
        assert_eq!(lv(&old), (0, 2, 3));
        // old booleans: true = default level, false = off
        let b = Enhancements::from_stored_in(TEST, [("E1", Stored::Bool(true)), ("E2", Stored::Bool(true)), ("E3", Stored::Bool(false))]);
        assert_eq!(lv(&b), (1, 2, 0));
        // a level past the entry's choices is clamped
        let c = Enhancements::from_stored_in(TEST, [("E1", Stored::Level(7))]);
        assert_eq!(c.level(Switch(0)), 1);
    }

    #[test]
    fn the_real_registry_is_well_formed() {
        for (i, e) in REGISTRY.iter().enumerate() {
            assert!(e.levels.len() >= 2 && e.levels.len() <= MAX_LEVELS, "{}", e.token);
            assert!((e.default_level as usize) < e.levels.len(), "{}", e.token);
            assert!(REGISTRY[..i].iter().all(|o| o.token != e.token));
        }
        assert!(REGISTRY.len() <= MAX_SWITCHES);
        assert_eq!(REGISTRY[E1.0 as usize].token, "E1");
        assert_eq!(REGISTRY[E6.0 as usize].token, "E6");
        assert_eq!(REGISTRY[E1.0 as usize].default_level, 2); // MSAA 4x
        assert_eq!(REGISTRY[E2.0 as usize].default_level, 3); // 16x
        assert!([E3, E4, E6].iter().all(|s| REGISTRY[s.0 as usize].default_level == 0));
        assert_eq!(REGISTRY[E5.0 as usize].default_level, 2); // draw distance: +50 % (2026-10-03)
        assert_eq!(REGISTRY[E7.0 as usize].token, "E7");
        assert_eq!(REGISTRY[E7.0 as usize].default_level, 1); // as the original
        assert_eq!(REGISTRY[E8.0 as usize].token, "E8");
        assert_eq!(REGISTRY[E8.0 as usize].default_level, 2); // clear coat: normal (ENH-COAT)
        assert_eq!(REGISTRY[E9.0 as usize].token, "E9");
        assert_eq!(REGISTRY[E12.0 as usize].token, "E12");
        assert_eq!(REGISTRY[E12.0 as usize].default_level, 1); // soft particles: on
        assert_eq!(REGISTRY[E10.0 as usize].token, "E10");
        assert_eq!(REGISTRY[E10.0 as usize].default_level, 1); // bloom: light
        assert_eq!(REGISTRY[E11.0 as usize].token, "E11");
        assert_eq!(REGISTRY[E11.0 as usize].default_level, 1); // bright sun: on
        assert_eq!(REGISTRY[E16.0 as usize].token, "E16");
        assert_eq!(REGISTRY[E15.0 as usize].token, "E15");
        assert_eq!(REGISTRY[E15.0 as usize].default_level, 2); // headlights: with shadows
        assert_eq!(REGISTRY[E14.0 as usize].token, "E14");
        assert_eq!(REGISTRY[E14.0 as usize].default_level, 1); // AO: weak
        assert_eq!(REGISTRY[E18.0 as usize].token, "E18");
        assert_eq!(REGISTRY[E18.0 as usize].default_level, 1); // particles in HDR: on
        assert_eq!(REGISTRY[E19.0 as usize].token, "E19");
        assert_eq!(REGISTRY[E19.0 as usize].default_level, 1); // fog and sky: on
        assert_eq!(REGISTRY[E20.0 as usize].token, "E20");
        assert_eq!(REGISTRY[E21.0 as usize].token, "E21");
        assert_eq!(REGISTRY[E21.0 as usize].default_level, 1); // eye adaptation: on (2026-10-03)
        assert_eq!(REGISTRY[E23.0 as usize].token, "E23");
        assert_eq!(REGISTRY[E23.0 as usize].default_level, 2); // street lamps: normal
        assert_eq!(REGISTRY[E24.0 as usize].token, "E24");
        assert_eq!(REGISTRY[E24.0 as usize].default_level, 1); // headlights in tunnels: on
        assert_eq!(REGISTRY[E25.0 as usize].token, "E25");
        assert_eq!(REGISTRY[E22.0 as usize].token, "E22");
        assert_eq!(REGISTRY[E22.0 as usize].default_level, 6); // frame rate limit: auto (2026-10-04)
        assert_eq!(REGISTRY[E27.0 as usize].token, "E27");
        assert_eq!(REGISTRY[E27.0 as usize].default_level, 0); // motion blur: off (taste)
        assert_eq!(REGISTRY[E28.0 as usize].token, "E28");
        assert_eq!(REGISTRY[E29.0 as usize].token, "E29");
        assert_eq!(REGISTRY[E29.0 as usize].default_level, 1); // camera shake: light
        assert_eq!(REGISTRY[E26.0 as usize].token, "E26");
        assert_eq!(REGISTRY[E26.0 as usize].default_level, 1); // wet road and rain: on
        // 2026-10-27: photo mode is always on (master's PORT-DEVIATION D46), no switch; an old "E30" is unknown
        assert!(!REGISTRY.iter().any(|e| e.token == "E30"));
        let mut unknown = Vec::new();
        assert_eq!(Enhancements::parse_override("E30,E31=1", &mut unknown).map(|e| (e.level(E31), e.any_on())), Some((1, true)));
        assert_eq!(unknown, vec!["E30".to_string()]);
        assert_eq!(REGISTRY[E31.0 as usize].token, "E31");
        assert_eq!(REGISTRY[E31.0 as usize].default_level, 0); // depth of field in replays: off
        assert_eq!(REGISTRY[E32.0 as usize].token, "E32");
        assert_eq!(REGISTRY[E32.0 as usize].default_level, 2); // global illumination: medium (2026-10-04)
        assert_eq!(REGISTRY[E35.0 as usize].token, "E35");
        assert_eq!(REGISTRY[E35.0 as usize].default_level, 0); // SMAA: off (cost vs a barely visible gain)
        assert_eq!(REGISTRY[E36.0 as usize].token, "E36");
        assert_eq!(REGISTRY[E36.0 as usize].default_level, 2); // haze and beams: medium (2026-10-04)
        // ENHANCEMENT E34: on every platform (indices stable), off by default, hidden / forced off where it has no backend
        assert_eq!(REGISTRY[E34.0 as usize].token, "E34");
        assert_eq!(REGISTRY[E34.0 as usize].default_level, 0);
        assert_eq!(REGISTRY[E34.0 as usize].levels.len(), 8);
        assert_eq!(REGISTRY[E34.0 as usize].levels.len() as u8, 4 + E34_TEMPORAL_LEVELS);
        assert_eq!(REGISTRY[E34.0 as usize].available(), cfg!(target_os = "macos"));
        let mut u = Vec::new();
        let set = Enhancements::parse_override("all,E34=2", &mut u).unwrap();
        assert_eq!(set.level(E34), if cfg!(target_os = "macos") { 2 } else { 0 });
        assert_eq!(u.is_empty(), cfg!(target_os = "macos"));
        assert_eq!(REGISTRY[E33.0 as usize].token, "E33");
        assert_eq!(REGISTRY[E33.0 as usize].default_level, 0); // HDR display: off until judged on the real screen
        assert_eq!(REGISTRY[E33.0 as usize].available(), cfg!(target_os = "macos"));
        // ENHANCEMENT E37: off by default, five levels (off, AA only, quality, balanced, performance); on every platform since ENH-UPSCALER
        assert_eq!(REGISTRY[E37.0 as usize].token, "E37");
        assert_eq!(REGISTRY[E37.0 as usize].default_level, 0);
        assert_eq!(REGISTRY[E37.0 as usize].levels.len(), 5);
        assert!(REGISTRY[E37.0 as usize].available());
        // ENHANCEMENT ENH-UPSCALER: E38 / E39 follow E37; E34 / E37 keep their levels (every index after 28 moved down by one when
        // E30 left the registry 2026-10-27: nothing stores an index)
        assert_eq!((E34.0, E37.0, E38.0, E39.0), (33, 35, 36, 37));
        assert_eq!(REGISTRY[E38.0 as usize].token, "E38");
        assert_eq!(REGISTRY[E39.0 as usize].token, "E39");
        assert_eq!(E44.0 as usize, REGISTRY.len() - 1); // ENHANCEMENT E44 is the last entry
        assert_eq!((E39.0, E40.0, E41.0, E42.0, E43.0, E44.0), (37, 38, 39, 40, 41, 42));
        assert_eq!(REGISTRY[E44.0 as usize].token, "E44");
        assert_eq!(REGISTRY[E44.0 as usize].default_level, 1); // lighthouse beam: on
        assert_eq!(REGISTRY[E44.0 as usize].levels.len(), 2);
        assert_eq!(REGISTRY[E43.0 as usize].token, "E43");
        assert_eq!(REGISTRY[E43.0 as usize].default_level, 1); // contact shadow: on
        assert_eq!(REGISTRY[E43.0 as usize].levels.len(), 2);
        assert_eq!(REGISTRY[E42.0 as usize].token, "E42");
        assert_eq!(REGISTRY[E42.0 as usize].default_level, 1); // rounded details: soft
        assert_eq!(REGISTRY[E42.0 as usize].levels.len(), 3);
        assert_eq!(REGISTRY[E41.0 as usize].token, "E41");
        assert_eq!(REGISTRY[E41.0 as usize].default_level, 1); // textures: on where a pack is found; off in the light preset
        assert_eq!(REGISTRY[E41.0 as usize].levels.len(), 2);
        assert_eq!(REGISTRY[E40.0 as usize].token, "E40");
        assert_eq!(REGISTRY[E40.0 as usize].default_level, 1); // raindrops: on, subtle
        assert_eq!(REGISTRY[E38.0 as usize].default_level, 0); // the upscaler is off by default
        assert_eq!(REGISTRY[E38.0 as usize].levels.len(), 3);
        assert_eq!(REGISTRY[E39.0 as usize].levels.len(), 5);
        assert_eq!(fsr_sharpness(REGISTRY[E39.0 as usize].default_level), Some(0.3)); // the old constant
    }

    #[test]
    fn upscaler_choice_and_effective_set() {
        // the stored levels of the two backends count only for the chosen one; the stored set keeps both
        let mut s = Enhancements::all();
        s.set_level(E34, 3);
        s.set_level(E37, 4);
        assert_eq!(s.effective().level(E34), 0);
        assert_eq!(s.effective().level(E37), 0); // E38 = off: no upscaler
        s.set_level(E38, UP_FSR);
        assert_eq!((s.effective().level(E34), s.effective().level(E37)), (0, 4));
        s.set_level(E38, UP_METALFX);
        assert_eq!(s.effective().level(E37), 0);
        assert_eq!(s.effective().level(E34), if cfg!(target_os = "macos") { 3 } else { 0 });
        assert_eq!((s.level(E34), s.level(E37)), (3, 4)); // the stored set is untouched
        // a chosen backend with no stored level takes the default (67 % temporal / 1.5x)
        let mut d = Enhancements::all();
        d.set_level(E38, UP_FSR);
        assert_eq!(d.effective().level(E37), 2);
        // the old tokens choose their backend: `E37=3` alone is FSR 1.7x with the old sharpness, `all,E37=2` too
        let mut u = Vec::new();
        let a = Enhancements::parse_override("E37=3", &mut u).unwrap();
        assert_eq!((a.level(E38), a.level(E37), a.level(E39)), (UP_FSR, 3, 2));
        let b = Enhancements::parse_override("all,E37=2", &mut u).unwrap();
        assert_eq!((b.level(E38), b.effective().level(E37)), (UP_FSR, 2));
        // an explicit E38 wins over the inference, and E39 named stays as named
        let c = Enhancements::parse_override("E37=3,E38=0,E39=0", &mut u).unwrap();
        assert_eq!((c.level(E38), c.effective().level(E37), c.level(E39)), (UP_OFF, 0, 0));
        let m = Enhancements::parse_override("E38=1", &mut u).unwrap();
        assert_eq!(m.effective().level(E34), if cfg!(target_os = "macos") { 2 } else { 0 });
        // defaults: no upscaler
        assert_eq!(Enhancements::all().level(E38), UP_OFF);
        assert!(!Enhancements::all().temporal_upscaler());
        // `describe` parses back with an upscaler on
        let e = Enhancements::parse_override("E37=2", &mut u).unwrap();
        assert_eq!(Enhancements::parse_override(&e.describe(), &mut u), Some(e));
        assert!(u.is_empty());
    }

    #[test]
    fn mfx_rows_map_onto_the_stored_level() {
        // mode + scale <-> E34's one level, all seven levels, both ways
        for level in 1..=7u8 {
            let (m, s) = (mfx_mode(level), mfx_scale_index(level));
            assert!(s < mfx_scale_count(m), "{level}");
            assert_eq!(mfx_level(m, s), level);
        }
        assert_eq!(mfx_mode(1), MfxMode::Temporal); // 1:1 is temporal only
        assert_eq!(mfx_level(MfxMode::Temporal, 0), 1);
        assert_eq!(mfx_mode(5), MfxMode::Simple);
        // switching the mode keeps the scale; the temporal 1:1 goes to simple 67 %
        assert_eq!([2, 3, 4].map(|l| mfx_switch_mode(l, MfxMode::Simple)), [5, 6, 7]);
        assert_eq!([5, 6, 7].map(|l| mfx_switch_mode(l, MfxMode::Temporal)), [2, 3, 4]);
        assert_eq!(mfx_switch_mode(1, MfxMode::Simple), 5);
        assert_eq!(mfx_switch_mode(3, MfxMode::Temporal), 3);
        // FSR's scale row: 1:1 / 1.5x / 1.7x / 2x = E37 levels 1..=4
        assert_eq!([1, 2, 3, 4].map(fsr_scale_index), [0, 1, 2, 3]);
        assert_eq!(fsr_sharpness(0), None);
        assert_eq!(fsr_sharpness(4), Some(0.8));
    }

    #[test]
    fn upscaler_choices_per_platform() {
        let c = upscaler_choices();
        assert_eq!(c.contains(&UP_METALFX), cfg!(target_os = "macos"));
        assert!(c.contains(&UP_FSR) && c[0] == UP_OFF);
    }

    /// the level maps give the values the renderer's and the app's selectors had inline, for every level
    /// of their entry (and level 0 = the original where it is asked).
    #[test]
    fn level_maps_are_the_old_selectors() {
        let lv = |s: Switch| 0..REGISTRY[s.0 as usize].levels.len() as u8;
        assert_eq!(lv(E1).map(msaa_samples_wanted).collect::<Vec<_>>(), [1, 2, 4, 8]);
        assert_eq!(lv(E3).map(render_scale).collect::<Vec<_>>(), [1.0, 0.5, 0.75, 1.25, 1.5, 2.0]);
        assert_eq!(lv(E4).map(fov_bonus_deg).collect::<Vec<_>>(), [0.0, 5.0, 10.0, 15.0, 20.0]);
        assert_eq!(lv(E5).map(draw_reach).collect::<Vec<_>>(), [1.0, 1.25, 1.5, 2.0]);
        assert_eq!(lv(E7).map(tonemap_mode).collect::<Vec<_>>(), [0, 0, 1]);
        assert_eq!((1..lv(E13).end).map(shaft_strength).collect::<Vec<_>>(), [0.45, 0.8]);
        assert_eq!((1..lv(E26).end).map(rain_gain).collect::<Vec<_>>(), [1.0, 1.5]);
        assert_eq!((1..lv(E23).end).map(street_lamp_pool).collect::<Vec<_>>(), [(0.75, 0.7), (1.0, 1.0)]);
    }

    /// The rows of [`DEPENDS`] name real entries, and [`Enhancements::resolved`] zeroes exactly what [`Enhancements::blocked`]
    /// shows inactive through a `Needs` row. Data only: it replaced a test that searched gpu.rs's source text for the gates.
    #[test]
    fn resolved_zeroes_the_dependents() {
        let known = |t: &str| REGISTRY.iter().any(|e| e.token == t);
        for d in DEPENDS {
            assert!(known(d.entry), "{}", d.entry);
            if let Cond::Needs(t) = d.cond {
                assert!(known(t) && t != d.entry, "{} needs {t}", d.entry);
            }
        }
        let sw = |t: &str| Switch(REGISTRY.iter().position(|e| e.token == t).unwrap() as u8);
        // the full set acts as stored: nothing it has on is blocked by a `Needs` row
        let all = Enhancements::all();
        assert_eq!(all.resolved(), all.effective());
        // the original, the light preset, the full set, and the full set with each prerequisite off in turn
        let mut sets = vec![Enhancements::ORIGINAL, Enhancements::lite(), all];
        for d in DEPENDS {
            if let Cond::Needs(t) = d.cond {
                let mut s = all;
                s.set_level(sw(t), 0);
                sets.push(s);
            }
        }
        for s in sets {
            let r = s.resolved();
            for d in DEPENDS {
                let e = sw(d.entry);
                if let Cond::Needs(t) = d.cond {
                    // a prerequisite at 0 zeroes the dependent
                    if r.level(sw(t)) == 0 {
                        assert_eq!(r.level(e), 0, "{} needs {t} in {}", d.entry, s.describe());
                    }
                }
                // what resolved() turns off is exactly what the panel shows blocked by a `Needs` row
                if s.effective().level(e) > 0 {
                    let needs_blocked = s.blocked(d.entry).is_some_and(|b| matches!(DEPENDS[b.dep].cond, Cond::Needs(_)));
                    assert_eq!(r.level(e) == 0, needs_blocked, "{} in {}", d.entry, s.describe());
                }
            }
            // it never turns anything on and is stable
            assert!(r.levels().iter().zip(s.effective().levels()).all(|(a, b)| a == b || *a == 0));
            assert_eq!(r.resolved(), r);
        }
        // the chain: E7 off takes the lens dirt (E28) through the bloom (E10) too; the stored level is kept
        let mut s = all;
        s.set_level(E7, 0);
        assert_eq!((s.resolved().level(E10), s.resolved().level(E28)), (0, 0));
        assert_eq!(s.level(E28), all.level(E28));
    }

    #[test]
    fn blocked_rows_follow_their_cause() {
        let mut s = Enhancements::all();
        assert_eq!(s.blocked("E10"), None);
        // HDR off blocks the bloom and the lens dirt (both rules), the stored levels stay
        s.set_level(E7, 0);
        let b = s.blocked("E10").expect("bloom needs HDR");
        assert_eq!(DEPENDS[b.dep].cond, Cond::Needs("E7"));
        assert!(s.blocked("E28").is_some() && s.blocked("E21").is_some() && s.blocked("E27").is_some() && s.blocked("E33").is_some());
        assert_eq!(s.level(E10), REGISTRY[E10.0 as usize].default_level);
        s.set_level(E7, 1);
        assert_eq!(s.blocked("E28"), None);
        // the lens dirt needs the bloom
        s.set_level(E10, 0);
        assert_eq!(DEPENDS[s.blocked("E28").unwrap().dep].cond, Cond::Needs("E10"));
        // the overcast shadows need the sun shadows, the car ambient the body reflections
        s.set_level(E9, 0);
        s.set_level(E8, 0);
        assert!(s.blocked("E25").is_some() && s.blocked("E16").is_some());
        // an upscaler: render scale is overridden by any; MSAA / SMAA only by a temporal one
        let mut u = Enhancements::all();
        assert!(u.blocked("E3").is_none() && u.blocked("E1").is_none());
        u.set_level(E38, UP_FSR);
        assert_eq!(u.blocked("E3").map(|b| b.by), Some(UP_FSR));
        assert!(u.blocked("E1").is_some() && u.blocked("E35").is_some());
        if cfg!(target_os = "macos") {
            u.set_level(E38, UP_METALFX);
            u.set_level(E34, 6); // simple, 59 %
            assert!(u.blocked("E3").is_some());
            assert!(u.blocked("E1").is_none() && u.blocked("E35").is_none());
            u.set_level(E34, 3); // temporal
            assert!(u.blocked("E1").is_some() && u.blocked("E35").is_some());
        }
    }
}
