//! [`REGISTRY`]: every enhancement's row (captions and help in English and Russian), in menu order.

use super::{Entry, Platforms, ON_OFF};

// To add one: append `Entry { token: "E45", .. }` here and `E45,` to `switches!` below (a token without an entry does not compile).
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
        platforms: Platforms::All,
    },
    Entry {
        token: "E2",
        name: ["anisotropic filtering", "анизотропная фильтрация"],
        help: ["sharper textures at a slant", "чёткие текстуры под углом"],
        levels: &[["off", "выкл"], ["4x", "4x"], ["8x", "8x"], ["16x", "16x"]],
        default_level: 3,
        platforms: Platforms::All,
    },
    Entry {
        token: "E3",
        name: ["render scale", "масштаб рендера"],
        help: ["off = the original 1:1. Not in effect while an upscaler (MetalFX / FSR 3.1) is on: it sets the internal size itself", "выкл = как в оригинале 1:1. Не действует, пока включен апскейлер (MetalFX / FSR 3.1): он сам задает внутренний размер"],
        levels: &[["off (100 %)", "выкл (100 %)"], ["50 %", "50 %"], ["75 %", "75 %"], ["125 %", "125 %"], ["150 %", "150 %"], ["200 %", "200 %"]],
        default_level: 0,
        platforms: Platforms::All,
    },
    Entry {
        token: "E4",
        name: ["field of view (FOV)", "угол обзора (FOV)"],
        help: ["a wider view than the original", "шире обзор, чем в оригинале"],
        levels: &[["off (original)", "выкл (оригинал)"], ["+5 deg", "+5°"], ["+10 deg", "+10°"], ["+15 deg", "+15°"], ["+20 deg", "+20°"]],
        default_level: 0,
        platforms: Platforms::All,
    },
    Entry {
        token: "E5",
        name: ["draw distance", "дальность прорисовки"],
        help: ["objects stay visible farther", "объекты видны дальше"],
        levels: &[["off (original)", "выкл (оригинал)"], ["+25 %", "+25 %"], ["+50 %", "+50 %"], ["+100 %", "+100 %"]],
        default_level: 2,
        platforms: Platforms::All,
    },
    Entry {
        token: "E6",
        name: ["frame output (vsync)", "вывод кадров (vsync)"],
        help: ["mailbox: latest frame, short queue, no tearing (not offered by Metal on macOS: then = off); immediate: no vsync, tearing, the front end is capped at 120 fps", "mailbox: свежий кадр, короткая очередь, без разрывов (на macOS Metal его нет: тогда = выкл); immediate: без vsync, возможны разрывы, меню не быстрее 120 кадров/с"],
        levels: &[["off (as now)", "выкл (как сейчас)"], ["mailbox (low lag)", "mailbox (мал. задержка)"], ["immediate (no vsync)", "immediate (без vsync)"]],
        default_level: 0,
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
    },
    Entry {
        token: "E9",
        name: ["sun shadows", "тени от солнца"],
        help: [
            "cars and roadside objects cast shadows from the sun (cascaded shadow maps); the car's blob shadow stays. In snowfall and rain the shadows fade with the track's own snow / rain amount (an overcast sky has no direct sun); the tracks whose data gives no sun direction (Foothills, Autobahn) have none",
            "машины и придорожные объекты отбрасывают тени от солнца; пятно под машиной остаётся. В снегопад и дождь тени слабеют по количеству снега / дождя из данных трассы (под сплошными тучами прямого солнца нет); на трассах, где данные не дают направления солнца (Предгорья, Автобан), их нет",
        ],
        levels: &[["off", "выкл"], ["low (1024, 3 cascades)", "низко (1024, 3 каскада)"], ["high (2048, 4 cascades)", "высоко (2048, 4 каскада)"]],
        default_level: 1,
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::MacOnly,
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
        platforms: Platforms::MacOnly,
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
        platforms: Platforms::All,
    },
    // ENHANCEMENT ENH-UPSCALER (registered last): the one choice of the upscaler backend; E34 / E37 keep their stored levels and
    // are the settings of the chosen backend (the panel shows only those rows). Level 1 (MetalFX) exists on macOS only.
    Entry {
        token: "E38",
        name: ["upscaler", "апскейлер"],
        help: [
            "off, MetalFX (macOS) or AMD FSR 3.1: the 3D scene is drawn smaller and enlarged to the window, which saves GPU work; the settings of the chosen upscaler follow below. While one is on, 'render scale' has no effect. The temporal modes (FSR 3.1, MetalFX 'temporal') work on the HDR image: they need 'HDR and tone mapping', without it they do nothing (MetalFX 'simple' works either way). On a Mac FSR works but is slower than MetalFX (and than no upscaler) — MetalFX is the Mac's own",
            "выкл, MetalFX (macOS) или AMD FSR 3.1: 3D-сцена рисуется меньше и растягивается до размера окна, это экономит работу видеокарты; ниже идут настройки выбранного апскейлера. Пока он включен, «масштаб рендера» не действует. Временные режимы (FSR 3.1, «временной» MetalFX) работают с HDR-изображением: им нужен «HDR и тонемап», без него они ничего не делают («простой» MetalFX работает в любом случае). На Mac FSR работает, но медленнее MetalFX (и медленнее, чем без апскейлера) — родной для Mac вариант MetalFX",
        ],
        levels: &[["off", "выкл"], ["MetalFX", "MetalFX"], ["FSR 3.1", "FSR 3.1"]],
        default_level: 0,
        platforms: Platforms::All,
    },
    // ENHANCEMENT ENH-UPSCALER: the upscaler's RCAS sharpening; a row of the FSR choice and (UPSCALE-SOFT) of the MetalFX one too:
    // FSR's own RCAS, for MetalFX (which has none) the port's RCAS pass (`sharpen.rs`). 0.3 is what E37 had as a constant.
    Entry {
        token: "E39",
        name: ["sharpness", "резкость"],
        help: [
            "contrast-adaptive sharpening (AMD's RCAS) after the upscale: it counters the softness of the upscaler's filter (FSR 3.1's own pass; for MetalFX, which has none, the same method as a pass of its own); too much rings on thin lines and foliage. off, or the strength 0.2 / 0.3 / 0.5 / 0.8",
            "контрастно-адаптивная резкость (RCAS от AMD) после апскейла: убирает мягкость фильтра апскейлера (у FSR 3.1 его собственный проход; для MetalFX, у которого своей нет, тот же метод отдельным проходом); слишком сильная даёт ореолы на тонких линиях и листве. Выкл или сила 0.2 / 0.3 / 0.5 / 0.8",
        ],
        levels: &[["off", "выкл"], ["0.2", "0.2"], ["0.3", "0.3"], ["0.5", "0.5"], ["0.8", "0.8"]],
        default_level: 2,
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
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
        platforms: Platforms::All,
    },
    // ENHANCEMENT E47: the cars' shadows from the street / tunnel lamps of E23 (registered LAST, after E44; the tokens E45 / E46 belonged to
    // removed trials). One small perspective depth map per lamp near the camera that has a car in its light, the cars as the only casters;
    // the map only attenuates that lamp's own added light. Needs E23 (its lamps).
    Entry {
        token: "E47",
        name: ["lamp shadows", "тени от фонарей"],
        help: [
            "the street lamps (E23) on night tracks and the tunnel lamps on every track cast the cars' shadows: under the four lamps nearest the camera that have a car in their light, a car darkens the lamp's pool on the road, the walls and the other cars on the side away from the lamp. Only the lamp's own added light is shadowed (night stays as dark as before, the original's baked light is untouched); the scenery casts nothing. Needs the street lamps (E23). Only the picture changes",
            "уличные фонари (E23) на ночных трассах и лампы тоннелей на любой трассе отбрасывают тени машин: под четырьмя ближайшими к камере фонарями, в свете которых есть машина, машина затеняет световое пятно фонаря на дороге, стенах и других машинах со стороны, противоположной фонарю. Затеняется только добавленный свет самого фонаря (ночь остаётся такой же тёмной, запечённый свет оригинала не меняется); окружение тени не отбрасывает. Нужны уличные фонари (E23). Меняется только картинка",
        ],
        levels: ON_OFF,
        default_level: 1,
        platforms: Platforms::All,
    },
    // ENHANCEMENT E48: the chase / heli cameras' slope lift held steady while the car stands across the road (registered LAST, after
    // E47). Camera state only: the lift's sample direction keeps its last side while the car's heading is near the slice's
    // perpendicular, where the wrong-way counter flips on every rock (race-logic §11.2, CAM-JIT2). Off = the original's camera exactly.
    Entry {
        token: "E48",
        name: ["steady chase camera on slopes", "плавная камера на склонах"],
        help: [
            "in the far chase and helicopter views the original lifts the camera by the road's slope, sampled ahead of or behind the car by whether it faces the wrong way; a car standing across a steep road flips that every time it rocks, and the camera swings up and down by most of a metre. With this on the slope is sampled on the same side until the car clearly turns along or against the road, so a car stuck across a slope keeps a still camera; while driving nothing changes. Off is the original's camera. Only the picture changes",
            "в дальних видах сзади и с вертолёта оригинал поднимает камеру по уклону дороги, измеренному впереди или позади машины в зависимости от того, едет ли она навстречу; машина, стоящая поперёк крутой дороги, переключает это при каждом покачивании, и камера качается вверх-вниз почти на метр. Со включённым улучшением уклон берётся с той же стороны, пока машина явно не развернётся вдоль дороги или против неё, так что у застрявшей поперёк склона машины камера стоит спокойно; в движении ничего не меняется. Выключено — камера оригинала. Меняется только картинка",
        ],
        levels: ON_OFF,
        default_level: 1,
        platforms: Platforms::All,
    },
    // ENHANCEMENT E49: reflections of nearby objects (registered after E48). E8's environment faces take the lamps' light (E23 / E15)
    // and the lamp flares, the player's own paint a second set of faces with the near cars, and in a tunnel the faces are read
    // through its covered box (a parallax-corrected lookup); NEAR-REFL2: on the player's car the original's sphere-map overlay (the
    // painted sky) shows the faces where they see geometry, not sky. Needs E8 (its cube). Off = E8's faces and paint exactly.
    Entry {
        token: "E49",
        name: ["reflections of nearby objects", "отражения ближних объектов"],
        help: [
            "the reflections in the cars' paint and glass (E8) show what is really near the car: the street and tunnel lamps and the headlights light the surroundings in the reflection and the lamps glint in it, the other cars within about 50 m appear in your own car's paint and glass (the other cars keep a reflection without cars, so none reflects itself), and inside a tunnel the walls, the ceiling and the lamp rows sit where they are and slide over the car as it drives (the reflection is laid onto the tunnel's box instead of an infinitely far surrounding). On your own car the original's painted sky in the paint and glass gives way to the real surroundings wherever something covers the sky: in a tunnel or under a rock it shows the ceiling, the rock and the lamps and moves with the car, in the open it stays the original's sky. The paint's colour and the strength of the reflection do not change. Needs the reflections (E8). Only the picture changes",
            "отражения в краске и стёклах машин (E8) показывают то, что действительно рядом с машиной: уличные и тоннельные фонари и фары освещают окружение в отражении, а лампы в нём блестят, другие машины в пределах примерно 50 м видны в краске и стёклах вашей машины (у остальных машин отражение без машин, так что ни одна не отражает саму себя), а в тоннеле стены, потолок и ряды ламп стоят на своих местах и скользят по машине при движении (отражение ложится на коробку тоннеля, а не на бесконечно далёкое окружение). На вашей машине нарисованное небо оригинала в краске и стёклах уступает настоящему окружению везде, где небо чем-то закрыто: в тоннеле или под скалой в нём видны потолок, скала и лампы, и оно движется вместе с машиной, а на открытой дороге остаётся небо оригинала. Цвет краски и сила отражения не меняются. Нужны отражения (E8). Меняется только картинка",
        ],
        levels: ON_OFF,
        default_level: 1,
        platforms: Platforms::All,
    },
    // ENHANCEMENT E50: moonlight on the night tracks (registered after E49). A faint cold directional light through E9's cascades (a second
    // light of them): soft shadows of the cars and the roadside objects on the baked night
    // colour, a weak cold direct term on the cars. Needs E9.
    Entry {
        token: "E50",
        name: ["moonlight", "лунный свет"],
        help: [
            "on the Monaco night tracks a faint cold moonlight: the cars, trees, posts and buildings cast soft moon shadows on the road and the cars take a weak cold light from the moon's side. The night stays as dark as before (the original's baked night light already holds the moon's glow; under the street lamps the moon shadow gives way to the lamp light). Monaco's sky has no moon, so the light comes from a chosen direction and stays weak («normal» draws as «weak» there). Industrial has no moonlight: its sky's clouds cover the moon (the track's sky data says so). Needs the sun shadows (E9). Only the picture changes",
            "на ночных трассах Монако слабый холодный лунный свет: машины, деревья, столбы и дома отбрасывают мягкие лунные тени на дорогу, а машины получают слабый холодный свет со стороны луны. Ночь остаётся такой же тёмной (запечённый ночной свет оригинала уже содержит свечение луны; под фонарями лунная тень уступает свету фонаря). На небе Монако луны нет, поэтому свет идёт с выбранной стороны и остаётся слабым («средне» там рисуется как «слабо»). На Индастриале лунного света нет: тучи на его небе закрывают луну (так сказано в данных неба трассы). Нужны тени от солнца (E9). Меняется только картинка",
        ],
        levels: &[["off", "выкл"], ["weak", "слабо"], ["normal", "средне"]],
        default_level: 2,
        platforms: Platforms::All,
    },
    // ENHANCEMENT E51: the cars' own sun shadow on dimly baked roads (registered after E50). E9's track receiver leaves a shadow out
    // where the baked colour is already dark (a baked shadow must not be darkened twice); a car's shadow is never baked into the
    // road, so a second, cars-only map of the near cascades lets the cars' shadows past that gate. Needs E9.
    // ALPS-CARSH / CARSH-FADE: level 1 scales each car's shadow by how much of the sun reaches that car (the car and the ground
    // round it in the scenery's shade: a gorge, a rock wall, a cliff), one value per car eased in time (`gpu/carsun.rs`); level 2
    // keeps the first rule (wherever the car's own map has it).
    Entry {
        token: "E51",
        name: ["car shadow on dark roads", "тень машины на тёмной дороге"],
        help: [
            "the cars' sun shadow is drawn in full on roads whose baked colour is already dim: the sun shadows (E9) leave the darkening out there so that a shadow baked into the road is not darkened twice, which also took the car's own shadow away; a car's shadow is never baked, so it now passes (the shadows of trees, rocks and buildings keep the rule). «where the sun reaches»: decided for each car as a whole - when the car and the ground round it stand in the sun shadow of a gorge, a rock wall or a cliff, its shadow fades out smoothly (and back in when it leaves); a house, a tree or a post that shades only the car leaves it; «always»: wherever the car blocks the sun, also inside such a shadow (the coast at dusk). Needs the sun shadows (E9). Only the picture changes",
            "солнечная тень машин рисуется полностью и на дороге, чей запечённый цвет уже тёмный: тени от солнца (E9) там не затемняют, чтобы не затемнить второй раз тень, запечённую в дорогу, и вместе с ней пропадала тень самой машины; тень машины никогда не запечена, поэтому теперь она рисуется (тени деревьев, скал и домов по-прежнему следуют правилу). «где светит солнце»: решается для машины целиком - когда машина и земля вокруг неё стоят в тени ущелья, скальной стены или утёса, её тень плавно гаснет (и так же плавно возвращается); дом, дерево или столб, затеняющие одну машину, тень не убирают; «всегда»: везде, где машина заслоняет солнце, и внутри такой тени (побережье в сумерках). Нужны тени от солнца (E9). Меняется только картинка",
        ],
        levels: &[["off", "выкл"], ["where the sun reaches", "где светит солнце"], ["always", "всегда"]],
        default_level: 1,
        platforms: Platforms::All,
    },
    // ENHANCEMENT E52: the track room's picture wall without seams (registered after E51; FE-WALLSEAM). The twelve prism panels of
    // `trackwall.crp` stand 0.0004-0.0011 apart (their pitch is 0.2069-0.2076 and a face is 0.2065 high), so a thin dark line shows
    // between them when the wall is drawn big; each panel is stretched along its own height by half the gap to its neighbours plus a hair,
    // so the faces meet. Render only, no data read or written. Off = the original's panels exactly.
    Entry {
        token: "E52",
        name: ["seamless picture wall", "стена с картинками без швов"],
        help: [
            "on the track selection screens the big wall of track pictures is built of twelve strips that stand a hair apart, so thin dark lines show between them (more of them the larger the window). With this on each strip is made a hair taller so the neighbours meet and the picture is whole; the photo does not shift or stretch visibly, and the strips still turn over as before. Off is the original's wall with its seams. Only the picture changes",
            "на экранах выбора трассы большая стена с картинками трасс собрана из двенадцати полос, которые стоят с крошечным зазором, поэтому между ними видны тонкие тёмные линии (чем больше окно, тем их больше). Со включённым улучшением каждая полоса чуть выше, так что соседние смыкаются и картинка цельная; фотография не сдвигается и заметно не растягивается, а полосы переворачиваются, как раньше. Выключено — стена оригинала со швами. Меняется только картинка",
        ],
        levels: ON_OFF,
        default_level: 1,
        platforms: Platforms::All,
    },
];
