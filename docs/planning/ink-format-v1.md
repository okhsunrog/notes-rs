# Ink format v1 — уточнённый проект, revision 6

Дата: 2026-09-06. Статус: проект для реализации и проверки, не опубликованный
стандарт. Core-модель и SQLite adapter реализованы; остальные этапы отмечены ниже.
Revision 6 документа не означает wire major 6:
wire v1 ещё не фиксировался. Числовые реестры ниже — согласованная исходная точка
реализации; замораживать их можно после реализации reader/writer и fixtures.

Этот документ заменяет предварительный INK-FORMAT-V1-DRAFT.md от 2026-09-06.
Основа сохранена: CBOR metadata, immutable column chunks, независимый выбор codec.
Изменения: точная миграция ширины/времени, контракт кисти, общий порядок объектов,
версии metadata, стабильные segments при compaction, portable reader, OCR metadata.
JSON перестаёт быть рабочим форматом после переключения приложения на новое хранилище.
Тестовые черновики пользователь разрешил удалить; импорт и fallback для них не реализуются.

Решение пользователя от 2026-09-06: основной compressor — crate `pco`,
`compression_level=8` (далее PCO-8). Это уровень encoder, не 8-битная точность
и не версия wire format. RAW остаётся несжатым контрольным/export профилем.
Решение о codec не меняет типы, точность samples, историю или размещение в SQLite/blobs.
Независимый крейт `crates/ink-format` реализует RAW/PCO blocks и CBOR envelopes.
Его публичный API не зависит от Tangleaf, SQLite, Tauri или BOOX. Реализованный wire
contract находится в `crates/ink-format/FORMAT.md` (копия для обмена —
`INKCHNK-WIRE-V1.md`). Этот документ дополнительно описывает проект полной модели
и её применение в Tangleaf; не все описанные части уже реализованы.
SQLite BLOB хранилище, история на 50 действий и фоновая compaction реализованы (§6.2).
OCR, синхронизация и полноценный portable container ещё не реализованы.

## 1. Область и термины

MUST/«обязан» — требование этого проекта; SHOULD/«рекомендуется» допускает явно
обоснованную альтернативу. Рекомендованные размеры, задержки и retention — настройки
реализации, не часть декодирования файла. Формат отделён от Android, BOOX и Tauri.

- Logical ID: стабильная личность document/page/object, 16 opaque bytes.
- Record ID: идентификатор неизменяемой версии metadata, также 16 bytes.
- Geometry/Profile/Session/Brush/Recognition ID: Record ID соответствующей записи.
- Segment ID: идентификатор неизменяемого фрагмента typed samples и их семантики.
- Chunk ID: идентификатор конкретной физической упаковки segments.
- Blob hash: SHA-256 точных физических bytes, 32 bytes, не Logical/Record ID.
- Ref: массив `[logical_id, record_id]` из двух byte strings по 16 bytes.

Нулевые ID запрещены. Новый ID не должен совпадать с существующим ID другого
содержимого: при импорте такое совпадение — conflict, не разрешение перезаписать.
UUID в строковом виде используются только в UI/диагностике. Geometry и chunks
не переписываются под прежним ID. Новая версия Stroke сохраняет logical object ID,
но получает новый Record ID. Равенство личности не означает равенства версии.

Пустая страница разрешена; сохранённый Stroke содержит >=1 sample. Пустые Geometry
и chunks writer не создаёт. Промежуточный жест, prediction, курсор и лассо-контур
не являются canonical ink. Отмена незавершённого жеста не создаёт пустой Stroke.

## 2. Модель metadata и реестр CBOR

### 2.1. Представление

Все metadata — CBOR. Поля схемы имеют unsigned integer keys. Значения integer
остаются integer; поля Float остаются float, даже если математически равны целому.
Float означает конечное значение binary64; writer использует кратчайшее CBOR float
представление, которое при расширении в f64 восстанавливает исходные биты, включая
знак нуля. NaN/Infinity запрещены. Little-endian относится к chunks/точным scalar
bytes, а не к собственному кодированию чисел CBOR.

Используется core deterministic CBOR из RFC 8949 §4.2.1: кратчайшие integer/length,
определённые длины, bytewise порядок закодированных ключей, без повторных ключей.
Дополнительный выбор протокола: никаких tags, undefined, иных simple values или
автоматического float→integer. Допустимы bool/null. UTF-8 не нормализуется молча.
Reader отклоняет некорректный/non-deterministic metadata record. Требование
проверяется по исходным bytes, до преобразования map библиотекой, теряющей дубликаты.

Неизвестные optional значения удерживаются как исходные CBOR items. Перезапись
знакомого поля не удаляет неизвестные поля. Для расширений используется map
`extension_id: opaque CBOR value`; extension_id — uint. Реестр приложения занимает
0x8000..0xffff, публичные ID ниже; unregistered writer в публичный диапазон не пишет.

Общая оболочка каждой записи:

| Key | Поле              | Тип                                                           |
| --: | ----------------- | ------------------------------------------------------------- |
|   0 | kind              | uint из таблицы ниже                                          |
|   1 | record_id         | bytes16                                                       |
|   2 | body              | map по таблице соответствующего kind                          |
|   3 | required_features | отсортированный массив уникальных uint, всегда присутствует   |
|   4 | extensions        | map, всегда присутствует, может быть пустой                   |
|   5 | dependencies      | отсортированный массив уникальных RecordId сильных ссылок     |
|   6 | resource_hashes   | отсортированный массив уникальных bytes32 сильных blob-ссылок |

dependencies/resource_hashes описывают прямые сильные ссылки body и extensions;
знакомые поля reader проверяет на соответствие этим спискам. Неизвестное optional
расширение обязано объявить свои сильные ссылки здесь: иначе GC/export не сможет
сохранить его данные. Граф dependencies ацикличен. Слабые provenance ссылки в него
не входят. Каталоги DocumentVersion — таблицы разрешения адресов, не дополнительные
сильные ссылки: его dependencies содержит только PageVersion records. Это позволяет
обойти и сохранить неизвестные optional records без знания их внутренней schema.

Record kinds: 1 DocumentVersion, 2 PageVersion, 3 StrokeVersion, 4 Geometry,
5 SourceProfile, 6 Session, 7 Brush, 8 Recognition, 9 TextCorrection.
10 ImageVersion и 11 TextVersion зарезервированы: их schemas/renderers не входят
в минимальный writer. Их появление требует зарегистрированного feature и реализации
контракта; зарезервированное имя само по себе не означает поддержку.

В body все перечисленные поля обязательны, кроме помеченных `?`. Необязательные
поля при отсутствии опускаются, а не заменяются null; явный null допустим только
там, где он указан в типе. Unknown body key >=0x8000 сохраняется как optional;
unknown key ниже 0x8000 требует известного required feature или отказа.

### 2.2. DocumentVersion и физический каталог

| Key | Поле             | Тип                                                      |
| --: | ---------------- | -------------------------------------------------------- |
|   0 | document_id      | bytes16                                                  |
|   1 | format_major     | uint, 1                                                  |
|   2 | format_minor     | uint, 0                                                  |
|   3 | pages            | упорядоченный массив Ref                                 |
|   4 | chunk_catalog    | map ChunkId -> `[sha256:bytes32, byte_length:u64]`       |
|   5 | segment_catalog  | map SegmentId -> `[chunk_id:bytes16, segment_index:u32]` |
|   6 | record_catalog   | map RecordId -> `[sha256:bytes32, byte_length:u64]`      |
|   7 | resource_catalog | map sha256:bytes32 -> byte_length:u64                    |

Record ID DocumentVersion — revision опубликованного snapshot. Его record_catalog
не содержит сам DocumentVersion: хеш корня передаётся снаружи, самоссылки нет.
Каталоги включают транзитивное замыкание сильных ссылок текущих pages. Historical
snapshots экспортируются отдельно, если явно заказан экспорт истории.

Geometry ссылается на Segment ID, **не на chunk/index**. segment_catalog данного
snapshot выбирает физическое размещение. Directory chunk обязан содержать ожидаемый
Segment ID по указанному index. Сам Segment ID не зависит от codec или размещения:
его повторное использование допустимо только при том же source profile, наборе
осей/dtype, validity и побитово равных logical values. DT включая segment-first=0
тоже входит в эту неизменяемую семантику. Новый codec/packing сохраняет Segment ID;
изменение samples, dtype или validity создаёт новый ID.

Это устраняет противоречие старого draft: compaction меняет физический каталог и
DocumentVersion, но оставляет Geometry, StrokeVersion и PageVersion прежними.
Старый snapshot продолжает ссылаться на старый каталог. Одинаковый chunk_id никогда
не обозначает разные bytes. Наличие ID внутри chunk исключает обещание автоматической
дедупликации независимо созданных одинаковых samples; копирование использует ссылки.

Эти каталоги — **логическое представление snapshot для экспорта**, не требование
пересериализовывать всю map при каждом сохранении. В рабочем storage они индексируются
транзакционными записями/версиями. Публикация и экспорт одного snapshot должны быть
согласованы; экспорт не собирает каталоги из разных моментов времени.

### 2.3. PageVersion

| Key | Поле                  | Тип                                           |
| --: | --------------------- | --------------------------------------------- |
|   0 | page_id               | bytes16                                       |
|   1 | width                 | Float >0                                      |
|   2 | height                | Float >0                                      |
|   3 | objects               | упорядоченный массив Ref, снизу вверх         |
|   4 | background            | Background map                                |
|   5 | recognition_ids       | массив RecordId                               |
|   6 | correction_ids        | массив RecordId                               |
|   7 | recognition_selection | map scope_key:bytes32 -> Recognition RecordId |

Оси страницы: +X вправо, +Y вниз, единица document-unit. DPI устройства не меняет
размер страницы или canonical coordinates. Drawing клипуется прямоугольником
`[0,width] × [0,height]`; перенос за границу страницы не удаляет скрытые samples.
Logical IDs объектов уникальны внутри страницы. Ref обязан указывать на запись
объекта с тем же logical ID; Geometry/Recognition не могут стоять в objects.

Общий objects заменяет `stroke_order + other_objects`. Порядок chunks никогда не
определяет рисование. Распознанный текст не рисуется сам: вставка видимого текста
в будущем создаёт TextVersion в objects и требует поддержки соответствующего feature.

Background: `{0:kind, 1:rgba}`; kind 0 plain, 1 grid. Для grid дополнительно
обязательны `2:step_x Float>0, 3:step_y Float>0, 4:origin [Float;2],
5:line_width Float>0, 6:line_rgba bytes4`.
Цвета — unpremultiplied sRGB bytes RGBA. Базовый фон должен иметь alpha=255.
Grid рисуется поверх фона, под objects; линии `x=origin_x+k*step_x` и аналогично Y
для целых k, строго внутри страницы, без отдельной рамки. Стиль — butt cap,
единый path/source-over. Экранная пиксельная сетка не меняет шаг document-unit.
Импорт grid: white, step 25×25, origin [0,0], line_width 1, line_rgba #777777ff.

### 2.4. StrokeVersion

| Key | Поле                 | Тип                            |
| --: | -------------------- | ------------------------------ |
|   0 | object_id            | bytes16                        |
|   1 | geometry_id          | RecordId                       |
|   2 | brush_id             | RecordId                       |
|   3 | rgba                 | bytes4, unpremultiplied sRGB   |
|   4 | base_width           | Float >0, geometry-local units |
|   5 | transform            | `[a,b,c,d,tx,ty]`, шесть Float |
|   6 | created_at_unix_ns ? | i64                            |
|   7 | updated_at_unix_ns ? | i64                            |

Толщина хранится с точностью f64; metadata малые, экономия f32 здесь не оправдывает
округление старых значений. Transform явно присутствует, даже identity.
`page_x=a*x+c*y+tx; page_y=b*x+d*y+ty`. Матрица применяется ко всему локальному
outline кисти, включая толщину; base_width поэтому в geometry-local units.
Это уточняет неоднозначные document units старого draft. У legacy identity transform
обе системы совпадают. Масштабирование меняет transform, не base_width и samples.
Все коэффициенты конечны; determinant ненулевой. Отражения допустимы. Writer
отклоняет операции с неинвертируемым/численно неустойчивым результатом, сохраняя
старое состояние. Reader не отбрасывает документ из-за произвольного UI zoom limit.

### 2.5. Geometry и parts

| Key | Поле              | Тип                                                    |
| --: | ----------------- | ------------------------------------------------------ |
|   0 | source_profile_id | RecordId                                               |
|   1 | timing            | null либо `{0:session_id, 1:start_tick:u64}`           |
|   2 | point_count       | u32, >0                                                |
|   3 | parts             | массив Part                                            |
|   4 | accuracy          | uint, 0 lossless; 1 reserved quantized-v1              |
|   5 | local_bounds ?    | `[min_x,min_y,max_x,max_y]` Float                      |
|   6 | edit_provenance ? | `{0:parent_geometry_ids [RecordId], 1:algorithm tstr}` |

Part: `{0:segment_id bytes16, 1:first_sample u32, 2:count u32>0,
3:first_t_tick u64}`. first_sample — смещение в результирующей Geometry, а не
поддиапазон segment. Parts покрывают [0,point_count) без пропусков/перекрытий;
каждый ссылается на полный segment с точно равным count. Первый first_sample=0.
Соседние parts продолжают один непрерывный штрих. Разрыв после pixel erase создаёт
отдельный Stroke/Geometry, иначе renderer соединил бы оставшиеся куски линией.

local_bounds перенесён из Stroke в Geometry: это optional консервативный кэш bounds
центров samples. Он не включает brush width. Reader проверяет/пересчитывает кэш перед
использованием для culling; несовпадение не даёт права спрятать существующий штрих.
Visual bounds вычисляются по brush outline + transform; AA padding зависит от renderer.
edit_provenance — слабые ссылки для происхождения, не требование удерживать всю историю.

## 3. Точность, профили и миграция

### 3.1. SourceProfile, Session и оси

SourceProfile body: `0:axes map semantic_id -> Axis, 1:provenance map,
2:tick_ns null|u64>0`. tick_ns задан, если профиль допускает DT.
Axis: `0:unit tstr, 1:representation uint, 2:convention tstr,
3:min Float?, 4:max Float?`. Representation 0 floating, 1 integer-level,
2 normalized, 3 enum. Bounds min/max обязательны для pressure integer-level,
max>min; normalized pressure имеет семантический диапазон [0,1].
Provenance map: `0:adapter_contract tstr, 1:platform tstr?, 2:api tstr?,
3:source_precision tstr?, 4:source_clock_kind uint?, 5:source_clock_origin tstr?`.
Для representation=1 допустимы исходные integer levels или производные дробные
levels в floating dtype; нормализация для brush использует те же min/max.
Representation=0 для pressure допустим только с min/max и тем же правилом нормализации.
Source clock kind: 0 unknown, 1 monotonic, 2 unix-wall, 3 derived.
Здесь фиксируется доступная семантика API, не модель устройства/аппаратный ID.

Session body: `0:tick_ns u64>0, 1:clock_kind uint=1 capture-monotonic,
2:wall_anchor map?, 3:adapter_contract tstr`.
Wall anchor: `0:session_tick u64, 1:unix_ns i64, 2:uncertainty_ns u64?`.
tick_ns Session и SourceProfile обязаны совпасть. Смена шкалы/clock reset создаёт
новую Session; любое отображение исходных часов явно определено adapter contract.
Без достоверной capture timeline используется timing=null, а не выдуманная Session.

| semantic_id | Ось                 | Допустимые logical dtype baseline |
| ----------: | ------------------- | --------------------------------- |
|           1 | X                   | F32/F64                           |
|           2 | Y                   | F32/F64                           |
|           3 | PRESSURE            | U16/U32/F32/F64                   |
|           4 | DT                  | U32/U64                           |
|           5 | TILT_X              | I8/I16/F32/F64                    |
|           6 | TILT_Y              | I8/I16/F32/F64                    |
|           7 | ROTATION            | U16/F32/F64                       |
|           8 | TANGENTIAL_PRESSURE | F32/F64                           |
|           9 | SIZE_MAJOR          | F32/F64                           |
|          10 | SIZE_MINOR          | F32/F64                           |
|          11 | SOURCE_TIME         | I64/F64                           |
|          12 | SAMPLE_KIND         | U8                                |

X/Y обязательны, конечны и all-valid. DT обязателен/all-valid при timing!=null,
отсутствует при timing=null. SAMPLE_KIND обязателен/all-valid: 0 measured,
1 synthetic-edit, 2 imported-unknown. Остальные оси могут отсутствовать или иметь
missing values. Неизвестная ось не заменяется нулями. DT первого sample каждого
segment равен 0; повторные timestamps разрешены.

`t_geometry(i) = part.first_t_tick + sum(segment.dt[1..i])`,
`t_session(i) = geometry.start_tick + t_geometry(i)`.
Первая part имеет first_t_tick=0; следующие anchors не меньше конца предыдущей part.
Арифметика проверяется на u64 overflow. При timing=null все first_t_tick=0.
Пауза на границе segments хранится в anchor, а не теряется с первым dt=0.

SOURCE_TIME сохраняет исходное число и шкалу, описанную Axis/Provenance. Это может
быть epoch-ms i64, исходный floating timestamp либо число с неизвестным origin.
Ось не объявляет часы монотонными и может сосуществовать с нормализованной DT timeline.
Одинаковый chunk объединяет только совместимые profile, tick_ns и наличие DT.

### 3.2. Исторический F64-профиль бенчмарка — без импорта в приложение

Решение пользователя от 2026-09-06: тестовые JSON-черновики не мигрировать.
При переключении рабочего storage приложение начинает новый документ; старый
JSON не является источником загрузки/fallback. Отдельный экспортированный образец
для бенчмарков сохраняется независимо от черновиков приложения.

Для воспроизводимости уже выполненных экспериментов профиль §3.2 сохраняет смысл:
X/Y/pressure/tilt и SOURCE_TIME представлены как F64 без округления; ширина Float
сохраняет f64 bits, transform identity. SOURCE_TIME unit=ms, clock_kind=unknown,
origin="legacy-unspecified", timing=null, DT отсутствует. SAMPLE_KIND=imported-unknown.
Случайная представимость конкретного значения как F32 не сужает dtype всей колонки.

Это точность относительно значений после прежнего JSON parsing, не исходных байтов
JSON и не исходного SDK: старый adapter нормализовал координаты/давление и применял
fallback давления. Новый код импорта ради этого корпуса не требуется.

### 3.3. Новый ввод и геометрическое редактирование

Первый adapter может сохранить текущую нормализованную модель f64 с явно названным
контрактом. Переход к native F32 coordinates/integer pressure — отдельный capture
profile, не скрытое свойство compressor. Указать mapping координат и обработку нулевого
давления до объявления native capture lossless. Pen SDK prediction в samples не пишется.

Pixel erase создаёт новые Geometry. Неизменённые samples сохраняются точно;
неизменённые целые segments переиспользуются. Новые точки на границах — synthetic-edit,
не аппаратные измерения. XY/pressure интерполируются по алгоритму edit_provenance.
Для tangleaf-linear-cut-v1: mix(a,b,t)=a+(b-a)\*t в f64, 0<=t<=1, без FMA;
нельзя молча сужать получившийся pressure/tilt до integer или F32. При необходимости
создаётся производный floating SourceProfile и новые segments с точным widening.
Точная точка t=0/1 копируется, а не пересчитывается; SOURCE_TIME нового synthetic
sample missing, исходных samples сохраняется. Axis-specific interpolation неизвестной
sample-aligned оси требует её поддержки; иначе операция отклоняется целиком.

Если есть DT, synthetic tick — ближайшее целое к a_tick+(b_tick-a_tick)\*t,
ties-to-even, где t трактуется как точное рациональное значение binary64; вычисление
не должно сначала сужать u64 ticks до JS Number. Начало полученной Geometry и anchors
перебазируются с сохранением этой capture timeline. При timing=null Session не создаётся.
Новая geometry accuracy=lossless означает отсутствие дополнительного codec loss,
а не отсутствие намеренного редактирования; edit_provenance/SAMPLE_KIND это различают.

## 4. Версионированная кисть и переносимость изображения

Brush body: `0:contract tstr="tangleaf-segment-v1", 1:contract_version uint=1,
2:parameters map={}`. Это data contract, не загружаемый исполняемый код.
Изменение алгоритма получает другое имя/версию; reader не подменяет неизвестную кисть.

Для samples P0..Pn-1 в порядке Geometry:

1. PRESSURE normalized: p в [0,1]; integer-level: p=(value-min)/(max-min).
   Floating representation с min/max использует ту же нормализацию, что levels.
   Missing pressure даёт p=0.5 только при рисовании. Нуль — действительное p=0,
   не сигнал продолжить предыдущее давление. Невалидное значение не clamp при чтении.
2. Нарисовать диск в P0 с diameter=base_width*(0.25+1.5*p0).
3. Для каждой пары Pi-1,Pi вычислить diameter=base_width*(0.25+1.5*(p_prev+p)/2).
   При разных XY рисовать прямой отрезок этой постоянной толщины с round caps;
   при совпадающих XY — диск. Дополнительного smoothing/Bezier/resampling нет.
4. Примитивы рисуются последовательно цветом Stroke.rgba в режиме source-over
   с обычной sRGB-композицией. Каждый primitive композитится отдельно; при alpha<255
   перекрытия могут быть темнее. Это часть контракта, не union всех примитивов.
5. Transform применяется ко всему outline: круг при неравномерном scale становится
   эллипсом. Давление и tilt не умножаются на affine matrix. Эта кисть не использует
   tilt/time/rotation для формы, но storage сохраняет оси.

При разрезании одного штриха на parts первый диск рисуется **один раз на Geometry**,
отрезок между последним sample предыдущей part и первым следующей сохраняется.
Первый sample части не дублируется в canonical data. Cached renderer обязан запросить
соседний sample, если начинает рисование внутри Geometry.

Контракт фиксирует геометрию и композицию, но не обещает одинаковые AA pixels разных
Canvas/Skia/GPU. Эталон геометрии проверяется численно; растровая миграция сравнивается
на одном backend при одинаковых размере, background, transform и color settings.
Fast BOOX Fountain — временный preview; canonical renderer остаётся указанной кистью.
Его отличия не компенсируются изменением сохранённых samples.

## 5. Совместимость и физический chunk

### 5.1. Обязательный portable reader

Feature registry: 1 core-column-ink-v1, 2 tangleaf-segment-v1,
3 source-time-v1, 4 sample-kind-v1, 5 recognition-v1.
Portable reader обязан поддерживать 1..4, plain/grid, X/Y/pressure/DT/tilt,
SOURCE_TIME и SAMPLE_KIND с перечисленными dtype. Оси 7..10 обязан сохранить opaque,
может не использовать при рисовании этой кистью. Feature 5 optional для UI, но его
записи/ссылки сохраняются при редактировании. Нельзя объявлять feature required,
если его неизвестность действительно не запрещает интерпретацию документа.

Обязательный decoder готовой реализации поддерживает RAW и выбранный PCO.
Контрольный RAW profile: codec wire version 1, outer=none, byte_shuffle=false,
constant=false; каждое valid значение представлено полностью обычными LE scalar
bytes, включая константные колонки. Поддерживаются all-valid/mixed/all-null.
RAW reader/writer остаётся первым проверяемым этапом и способом несжатого экспорта.
Описания shuffle, constant и outer ниже резервируют будущие extensions и сами по себе
не включают их в writer.

PCO=2 выбран основным codec, encoder compression_level=8, outer=none,
accuracy=lossless. Уровень задаётся явно, а не через неявный default crate.
QUOIN=1 остаётся reserved и не включается в обязательный decoder.
Outer LZ4=2 reserved optional; raw block не является frame.
Пара codec_id/codec_wire_version определяет совместимость decoder отдельно от
уровня encoder и версии crate. Реализованный mapping: PCO codec_id=2, wire=1,
outer=0, params={} (CBOR a0), ровно один complete standalone PCO stream на колонку.
Все десять dtype поддерживаются напрямую, logical_dtype=storage_dtype; 8-bit numeric
support включён. Encoder: pco 1.0.3, level 8, mode/delta Auto, equal pages up to 2^18.
Decoder ограничивает суммарное число значений declared valid_count до выделения
output, проверяет dtype/count, конец stream и отсутствие trailing bytes. Fixtures
лежат в `crates/ink-format/tests/fixtures`. Наличие codec ID не означает поддержку
любой будущей версии PCO stream; неизвестная версия даёт ошибку.

RAW export декодирует PCO без изменения logical dtype/bits/Segment IDs и записывает
полные колонки, без outer/shuffle/constant. Unsupported required column/codec — явная ошибка.

Unknown required feature/major/flag/brush/visible object kind запрещает обычное
редактирование/рендер документа. Можно показать отдельный verified preview как preview,
но нельзя выдавать его за редактируемый оригинал. Неизвестную optional колонку или
metadata сохраняют; операция, способная изменить её семантику, должна понимать её
или отказаться. Unknown optional не означает «можно удалить».

### 5.2. Framing

Header/SegmentEntry/ColumnEntry ниже сохраняют 80/32/64 bytes исходного draft.
Все числа в framing little-endian, записываются явно, без struct padding/transmute.
Directory начинается на offset 80; содержит сначала все SegmentEntry, затем все
ColumnEntry, затем params/validity blobs. Payload начинается на 80+directory_bytes.
Порядок ColumnEntry строго по semantic_id; дубликаты запрещены. Ranges внутри своих
областей, не пересекаются; допускается только нулевой padding. Suffix запрещён.

Header:

| Offset | Bytes | Поле                              |
| -----: | ----: | --------------------------------- |
|      0 |     8 | magic ASCII INKCHNK + NUL         |
|      8 |     2 | major=1                           |
|     10 |     2 | minor=0                           |
|     12 |     4 | required_flags=0                  |
|     16 |    16 | chunk_id                          |
|     32 |    16 | source_profile_id                 |
|     48 |     4 | point_count >0                    |
|     52 |     4 | segment_count >0                  |
|     56 |     2 | column_count >0                   |
|     58 |     2 | reserved=0                        |
|     60 |     4 | directory_bytes                   |
|     64 |     8 | payload_bytes                     |
|     72 |     4 | header_crc32 по bytes[0..72)      |
|     76 |     4 | directory_crc32 по всей directory |

SegmentEntry: offset 0 bytes16 segment_id; 16 u32 count>0; 20 u32 flags=0;
24 u64 reserved=0. row_start — сумма предыдущих count; sum(count)=point_count.
Segment ID не повторяется в одном chunk. В разных упаковках он может повторяться
с идентичной семантикой, но каталог snapshot выбирает ровно одно размещение.

ColumnEntry:

| Offset | Bytes | Поле                                               |
| -----: | ----: | -------------------------------------------------- |
|      0 |     2 | semantic_id                                        |
|      2 |     1 | logical_dtype                                      |
|      3 |     1 | storage_dtype                                      |
|      4 |     2 | codec_id                                           |
|      6 |     2 | codec_wire_version                                 |
|      8 |     2 | outer_id                                           |
|     10 |     2 | flags                                              |
|     12 |     4 | logical_count = point_count                        |
|     16 |     8 | payload_offset от начала chunk                     |
|     24 |     4 | payload_bytes после outer                          |
|     28 |     4 | decoded_bytes = valid_count\*sizeof(storage_dtype) |
|     32 |     4 | params_offset от начала chunk                      |
|     36 |     4 | params_bytes                                       |
|     40 |     4 | validity_offset от начала chunk                    |
|     44 |     4 | validity_bytes                                     |
|     48 |     4 | payload_crc32 физических bytes                     |
|     52 |     4 | codec_bytes после снятия outer                     |
|     56 |     8 | reserved=0                                         |

Dtype: 1 U8, 2 I8, 3 U16, 4 I16, 5 U32, 6 I32, 7 U64, 8 I64, 9 F32, 10 F64.
Flags bits0..1: 0 all-valid, 1 mixed, 2 all-null, 3 reserved; bit2 constant,
bit3 lossy, bit4 required; остальные 0. X/Y/DT/SAMPLE_KIND имеют required=1.
SOURCE_TIME optional для render, но требует feature 3 и сохраняется при edit/export.

Mixed bitmap raw LSB-first, ceil(N/8), tail bits=0, строго 0<popcount<N.
Codec получает только valid samples по порядку строк. All-valid/all-null не имеют
bitmap. All-null запрещён для обязательных осей; count N>0, valid_count=0,
decoded/codec/payload bytes=0. Для всех отсутствующих ranges offset/length=0/0;
CRC пустого payload=0. Не бывает одновременно constant и all-null.

RAW params: map ключ 0 byte_shuffle bool, ключ 1 constant_scalar bytes?;
ключ 0 всегда присутствует. При constant scalar обязателен, ровно sizeof(storage_dtype)
LE bytes; shuffle=false, codec=RAW wire=1, outer=none, payload/codec bytes=0.
decoded_bytes всё равно valid_count\*sizeof(storage_dtype). Constant mixed допустим;
повторяется одно и то же побитовое значение для всех valid positions. +0/-0 различны.
All-null использует RAW wire=1, outer=none, params={0:false}, без scalar.

Обычный RAW stream — LE scalars. При shuffle вход b[i*w+j] преобразуется в
shuffled[j*n+i], n=valid_count, w=storage width. Обратное действие строго обратное.
Если outer=none: payload_bytes=codec_bytes=decoded_bytes. С Zstd распаковка обязана
дать ровно codec_bytes=decoded_bytes, исчерпав ровно один frame; concatenated frames,
skippable frames, dictionary requirement и trailing bytes в portable profile запрещены.
Window/output allocations ограничиваются до распаковки. CRC-32/ISO-HDLC проверяет
header/directory/payload; SHA-256 из каталога проверяет весь chunk. Это integrity,
не аутентификация автора.

Portable writer сохраняет logical_dtype=storage_dtype. Typed integer widening
для optional codec требует зарегистрированной точной пары, проверки диапазона и
обратимости. Float-bit reinterpretation как публичная U32 колонка не допускается.
Quantization и дополнительные delta predictors не активируются codec level:
им нужны отдельные зарегистрированные accuracy/representation contracts и fixtures.

## 6. Версии, Undo, публикация и GC

### 6.1. Что меняется при операции

Таблица описывает целевую семантику portable editor. Текущий scratch-sheet adapter
пока материализует move/scale/copy как новые normalized samples; поддержка affine
операций без переписывания Geometry — отдельная последующая оптимизация.

| Операция           | Новые immutable записи                                        | Что переиспользуется                                |
| ------------------ | ------------------------------------------------------------- | --------------------------------------------------- |
| Новый штрих        | samples/segments/chunks, Geometry, StrokeVersion, PageVersion | остальная страница                                  |
| Move/rotate/scale  | StrokeVersion с новой matrix, PageVersion                     | Geometry и chunks                                   |
| Цвет/толщина       | StrokeVersion, PageVersion                                    | Geometry и chunks                                   |
| Copy               | новый object_id и StrokeVersion, PageVersion                  | Geometry и chunks                                   |
| Stroke/lasso erase | PageVersion без удалённых Ref                                 | старые версии для Undo                              |
| Pixel erase        | новые fragments/Geometry/StrokeVersion/PageVersion            | целые неизменённые segments, если profile совместим |
| Compaction         | новые chunks и физический каталог DocumentVersion             | все semantic records/Segment IDs                    |

Каждая публикация создаёт новый DocumentVersion. Один gesture = одна history entry;
preview frames и compaction не создают пользовательских Undo entries. Для первого
редактора scope истории — локальная ink page. History entry хранит before/after Page Ref,
а не только geometry/chunk IDs. Undo публикует before Page Ref в новом root, сохраняя
остальные страницы; Redo аналогично after. После Undo новая правка очищает redo branch.
GC roots включают обе стороны удерживаемых history entries. History в JSON не нужна.

Для каждой стороны history backend также удерживает catalog snapshot/lease, позволяющий
разрешить все её Segment IDs, даже если они отсутствуют в текущей странице. Одной пары
Page Ref без доступного физического каталога недостаточно. При Undo новая transaction
переносит нужные mappings из этого snapshot в актуальный root; остальные pages и их
mappings сохраняются. Альтернатива — версионированный индекс всех pinned Segment IDs,
обеспечивающий ту же гарантию в тесте delete -> compaction -> GC -> Undo.

Проверка expected head/CAS не даёт старому IPC save перезаписать новое состояние.
Compaction имеет отдельную физическую revision: ожидаемая semantic Page Ref не
становится устаревшей только из-за переупаковки; transaction подбирает актуальный
catalog. Настоящая конкурирующая правка страницы даёт conflict. Синхронизация не
решается last-writer-wins заменой root: отдельный будущий adapter к notes-core ops
обязан определить merge и guarded undo; wire v1 не объявляет этот adapter готовым.

### 6.2. Storage в Tangleaf

Первая реализация: отдельная device-local `handwriting/ink-v1.sqlite3`.
И canonical CBOR metadata, и INKCHNK с PCO-8 хранятся как SQLite BLOB.
`ink_records` и `ink_chunks` содержат immutable payload по ID; `ink_head`
содержит текущий Document root и уникальную revision пользовательского перехода. Core-крейт не зависит от SQLite.
Отдельных файлов chunks и интеграции с notes-blob на этом этапе нет.
Экспорт полного документа в будущем сериализует согласованный snapshot;
сам INKCHNK не является самодостаточной заметкой.

Последовательность публикации:

1. Сериализованный blocking worker получает patch и expected root revision;
   pen callback не занимается сжатием или SQLite.
2. BEGIN IMMEDIATE; проверяется текущая revision и целостность snapshot.
3. Для изменённых штрихов создаются новые geometry/stroke records и PCO-8 chunks.
   Неизменённые chunks переиспользуются без повторного сжатия. Обновляются каталоги.
4. В той же транзакции записываются новые BLOB и переключается head.
5. Commit, затем durable ack. Ошибка до commit откатывает и данные, и head.

Используется WAL+synchronous=FULL до начала transaction. Настройки общей базы
notes-core не изменяются. Результат зависит от исправной реализации fsync/VFS/device;
обычный kill процесса не эквивалентен power-cut test.

Adapter сохраняет одну экспериментальную страницу и до 50 отменяемых действий.
`ink_history(seq, root_id)` удерживает до 51 snapshot; `ink_cursor` задаёт выбранное
состояние. Каждый завершённый жест получает отдельный durable snapshot; очередь
фронтенда сохраняет отдельные жесты внутри удерживаемого окна истории. При длительном
сбое IO очередь ограничена текущей попыткой и последним 51 состоянием: более старые
промежуточные состояния могут истечь, итоговый рисунок сохраняется полностью. Undo/Redo сначала дожидается очереди, затем
атомарно перемещает cursor/head и выдаёт новый transition token. Это предотвращает
ABA при возвращении к прежнему состоянию. Новая правка после Undo удаляет redo-ветку.
GC в той же транзакции учитывает все оставшиеся history roots.

Schema 2 обновляет существующую SQLite-страницу, принимая её за начальное состояние
истории; старую историю восстановить невозможно. JSON import/fallback не добавляется.

Writer создаёт один chunk на изменённый штрих. Через 3 секунды без новых запросов
сохранения/истории запускается фоновая переупаковка. Она объединяет segments всех
удерживаемых состояний по совместимому profile/layout, не меняя IDs, samples,
Geometry/Stroke/Page records и границ действий. Target — 250000 points, максимум
65536 segments в chunk; target больше текущего лимита страницы (150000 points). Исторические версии
могут распределить её segments по нескольким chunks. Выбор target относится к adapter, не к core формату.

Декодирование/PCO-8 выполняется вне store mutex и write transaction. Подготовка читает
согласованный snapshot; публикация проверяет неизменность history roots и transition
token. При конкурирующей правке результат отбрасывается. Все каталоги истории и head
переключаются на новые blocks в одной транзакции; transition token при этом прежний.
Удаление старых blocks происходит лишь после обновления всех удерживаемых roots.
Общие колонки при открытии страницы декодируются один раз на chunk.

Задание ограничено 128 MiB encoded input и 128 MiB суммарных decoded columns;
это не hard process RSS limit. Более крупные задания пока откладываются. Публикация
происходит только при уменьшении суммарных encoded chunks и сохранении per-snapshot
лимита 64 MiB. История с многими состояниями может занимать существенно больше места,
чем один snapshot. Уменьшение SQLite-файла/VACUUM отдельно не выполняется: освобождённые
страницы переиспользуются. Export/sync pins нужно добавить до внедрения этих механизмов.

### 6.3. Сборка мусора и снимки

GC учитывает активные documents/drafts, before/after Undo/Redo, экспортные/read leases,
sync/outbox pins при появлении sync, explicit recovery pins и любые будущие import sessions.
Последовательность: serialize reference check с writers -> mark unreachable -> delete
только проверенные blobs/records. Нельзя проверять только таблицу attachments:
существующий attachment GC не знает о будущих ink references и требует интеграции.
До такой интеграции ink blobs не включаются в его candidate set.

Reader держит lease на snapshot/catalog до завершения чтения. Compaction публикует
новое размещение атомарно; старое удаляется только после освобождения leases/roots.
Unknown optional sample-aligned columns переносятся побитово; compressor, который
их не умеет декодировать, оставляет такой chunk на месте. Возраст файла сам по себе
не доказывает, что он orphan. Preview cache не удерживает canonical history навсегда.

## 7. Сохранение коротких штрихов и измерения

Framing cost = 80 + 32*S + 64*C + params + validity + padding.
Для одного segment/6 columns это 496 bytes до payload; с обязательным SAMPLE_KIND
7 columns — 560 bytes. Каталоги, CBOR records, filesystem allocation и история
считаются дополнительно. Constant detection полезна, но не отменяет descriptor cost.

Политика первого writer:

- Завершённый штрих ставится в последовательную очередь сохранения сразу. Не ждать
  накопления 32 KiB для первого durable ack. Несколько уже ожидающих операций можно
  записать одной transaction, сохранив отдельные history entries.
- Незавершённый штрих находится в памяти; UI не говорит «сохранено» до ack. При
  приостановке приложения сохраняются все завершённые операции. Durable streaming
  незавершённого штриха — отдельное расширение, не скрытое обещание v1.
- Размер chunk выбирает storage adapter, крейт не задаёт target. Бенчмарк страницы
  113877 points показал для PCO-8: whole-page 339345 bytes, 64 KiB target 500415,
  16 KiB target 864231. Поэтому прежние 32 KiB не считаются default: для compaction
  проверить whole-page и >=256 KiB, отдельно измеряя задержку foreground saves.
  Совместимые мелкие segments пакуются вместе; длинный stroke делится без потери
  граничного интервала и соединяющего отрезка. Новые маленькие chunks допустимы.
- Контрольный writer/export — RAW без outer/shuffle/constant. Даже очевидные
  константы хранятся целиком для независимых codec benchmarks.
  Основной encoder — PCO level 8; wire mapping и начальные fixtures реализованы.
  Сжатие выполняется worker, не callback пера; размещение и размер chunks
  проверяются отдельно на устройстве. Внешнее дополнительное сжатие не включается.
- Compaction выполняется в фоне после опустошения foreground очереди; рекомендуемый
  старт после 2 секунд без input, ограниченная работа с проверкой отмены между chunks.
  Она не блокирует новый жест. Никакой blanket-перезаписи целой страницы после каждого слова.
- Сначала переупаковывать страницы с >=8 мелкими chunks (<8 KiB decoded каждый) или
  большим количеством unreachable segments. Это стартовая эвристика, не доказанный optimum.
- Две метрики размера: logical bytes текущего snapshot и фактически занятое место
  с history/orphans/SQLite/WAL/файловыми блоками. Сжатие payload не равно экономии диска.

Bench на desktop и BOOX: empty page, одно слово, 100 коротких штрихов, большой штрих,
плотная страница, смешанные profiles, 100 перемещений, pixel erase, 100 Undo/Redo,
чтение одного stroke/страницы, GC/compaction с открытым snapshot. Измерять p50/p95
pen-up→durable ack, decode/open, bytes written, memory peak и CPU. Отдельно filesystem
fsync, SQLite commit и влияние на pen latency. Старые in-memory codec tables не
доказывают эти результаты. При медленном FULL сначала batch уже ожидающих операций,
а не ослаблять гарантию без отражения в статусе сохранения.

## 8. Распознавание, исправления и поиск

### 8.1. Неизменяемый результат

Recognition body:

| Key | Поле                   | Тип                                    |
| --: | ---------------------- | -------------------------------------- |
|   0 | source_page            | Ref, provenance weak reference         |
|   1 | scope                  | uint: 0 whole-page, 1 explicit-objects |
|   2 | source_objects         | массив Ref в page order                |
|   3 | appearance_fingerprint | bytes32                                |
|   4 | input_image            | InputImage map                         |
|   5 | engine                 | Engine map                             |
|   6 | languages              | массив tstr, language tags либо "und"  |
|   7 | text                   | UTF-8 tstr                             |
|   8 | regions                | массив Region, может быть пустым       |

InputImage: `0:sha256 bytes32, 1:width u32>0, 2:height u32>0,
3:pixel_to_page [Float;6], 4:render_contract tstr, 5:stored bool`.
Если stored=true, resource_catalog содержит blob; иначе hash остаётся provenance,
отсутствие изображения не делает ink нечитаемым. Не хранить credentials/API key.
Engine: `0:provider tstr, 1:model tstr, 2:model_revision tstr|null,
3:prompt_sha256 bytes32, 4:options_sha256 bytes32, 5:run_at_unix_ns i64?`.
Если точная версия модели недоступна, null, не выдуманный номер.
Region: `0:utf8_start u32, 1:utf8_length u32, 2:polygon [[Float,Float]],
3:confidence Float?`; polygon в page coordinates. Диапазоны лежат на UTF-8 границах
в text; порядок массива — reading order; confidence при наличии 0..1, без синтеза.
Отсутствие boxes у VLM допустимо. Байтовые offsets нельзя трактовать как JS UTF-16.

Recognition никогда не меняет исходные strokes. «Заменить рисунок текстом» — отдельная
пользовательская операция с Undo, создающая TextVersion после реализации text feature.
Для первого релиза достаточно скрытых metadata и индекса поиска.

### 8.2. Когда результат устарел

Fingerprint-v1 = SHA-256 deterministic CBOR массива:
`["ink-appearance-v1", scope, page_width, page_height, Background,
ordered_visual_objects]`.
Для stroke visual object = `[object_id, geometry_id, brush_contract, brush_version,
brush_parameters, rgba, base_width, transform]`. Для explicit scope берутся только
выбранные объекты в актуальном page order; фон/размер всё равно включены. Для whole-page
берутся все видимые kinds: неизвестный renderer kind запрещает создание fingerprint-v1.
Новые kinds добавляют версии fingerprint/feature, а не меняют смысл старой формулы.

Здесь Geometry ID неизменяем и переживает compaction; matrix и оформление включены.
Record IDs Stroke/Page, физические Chunk IDs, record catalogs, OCR metadata и timestamps
редактирования не входят. Поэтому compaction/запись OCR/служебная revision не делают
OCR устаревшим, а перемещение, изменение формы, цвета, фона или порядка делают.
Это консервативная первая политика: даже общий перенос выбранного слова требует
переоценки. Оптимизация translation-invariant recognition требует отдельного контракта.
Изменение geometry только по неиспользуемой кистью оси также может консервативно
инвалидировать OCR. Hash input_image дополнительно фиксирует точный запрос к модели.

До публикации результата worker сверяет fingerprint текущего scope. Поздний результат
сохраняется как stale, не перезаписывает актуальный. Статус fresh/stale вычисляется,
а не принимается на веру из сохранённого bool. Ссылки source_page/source_objects слабые:
для поиска нужен text, не удержание всех старых страниц навсегда. Explicit reproducibility
pin может отдельно удерживать исходный snapshot и input_image.

### 8.3. Пользовательский текст

TextCorrection body: `0:recognition_id RecordId (strong), 1:text tstr,
2:regions [Region], 3:author_kind uint=0 user, 4:created_at_unix_ns i64?`.
Пользовательская правка создаёт новую запись; исходный ответ модели сохраняется.
Если новые диапазоны слов не пересчитаны, regions=[]; старые offsets не наследуются.
Page correction_ids хранит не более одной выбранной коррекции на Recognition.
Regions и engine fingerprint не являются доказательством истинности текста.

Индекс поиска производный и восстанавливаемый. Для свежего выбранного результата
индексировать correction.text при наличии, иначе Recognition.text; не дублировать
один и тот же текст двумя записями. Для stale машинного результата — исключить из
обычной выдачи и предложить повторное распознавание. Пользовательский correction.text
сохранять в поиске как авторский текст с пометкой stale source; новая модель не стирает
его автоматически.

Выбранная запись задана Page.recognition_selection, её ID обязан присутствовать
в recognition_ids. scope_key = SHA-256 deterministic CBOR
`["ink-scope-v1", page_id, scope, sorted_unique_logical_object_ids]`, где для
whole-page массив пустой. Для explicit scope он непустой. Исторические версии объектов
в ключ не входят. Поздний stale ответ добавляется в recognition_ids, но не меняет
выбор. Первый свежий ответ может выбираться автоматически, последующие не вытесняют
пользовательскую коррекцию без явного действия. Пересекающиеся scopes могут давать
отдельные результаты с provenance. Все correction_ids удерживают авторский текст,
даже если соответствующий Recognition больше не выбран; индекс дедуплицирует по
Correction ID и не удаляет его при автоматическом выборе нового результата.

## 9. Ограничения и критерии готовности

Chunk hard limits сохраняются: <=64 columns, <=65536 segments, <=1_000_000 samples,
<=64 MiB physical chunk, <=16 MiB directory, <=64 MiB decoded numeric bytes.
Все offsets/суммы/counts checked до allocation. Params одной колонки <=64 KiB.
Metadata record <=16 MiB, nesting <=32. Initial desktop/BOOX import budget:
<=256 MiB суммарных CBOR metadata, <=1_000_000 references, decode working set <=128 MiB.
Это resource policy, не обещание открыть документ любого размера: превышение даёт
понятный limit error, без truncation. Большие данные читаются лениво. Текущий лимит
редактора 150000 samples отдельно пересматривается после bench, не снимается молча.

Словарь ошибок: invalid_format, unsupported_major, unsupported_feature,
unsupported_codec, unsupported_brush, integrity_error, resource_limit,
revision_conflict, io_error. Ошибка не заменяет документ пустым и не включает JSON fallback.

Обязательные fixtures/integration tests перед включением нового storage:

| Группа     | Что должно быть доказано                                                                                               |
| ---------- | ---------------------------------------------------------------------------------------------------------------------- |
| Precision  | f64 bits всех полей, ширины после scale, +0/-0, дробное время, порядок/UUID/grid                                       |
| Brush      | dot, repeated XY, zero/missing pressure, mixed integer/float profiles, alpha overlaps, nonuniform transform, part seam |
| Columns    | RAW и PCO-8 bit-exact roundtrip; mixed/all-null; malformed flags/ranges/counts/CRC/hash; unsupported wire refusal      |
| Metadata   | deterministic CBOR, duplicate keys, unknown optional roundtrip, unknown required refusal, u64 beyond JS safe integer   |
| Editing    | move/copy без новых chunks; pixel cut без соединения через разрыв; synthetic provenance; Undo с matrix/style/order     |
| Catalog    | compaction меняет размещение, не Geometry/Page/appearance fingerprint; lease удерживает старый chunk                   |
| Durability | fault injection до/после установки blobs, до/после commit/ack, retry idempotence, no missing referenced blob           |
| OCR        | stale late response, layout/background invalidation, compaction stability, correction preservation, UTF-8 offsets      |
| Device     | свежие слова/переворот пера/ластики/лассо/Undo/перезапуск, latency и память на BOOX                                    |

Проверка fault injection должна различать crash процесса и power-loss гарантию.
В независимом крейте прошли 13 integration tests и пример из документации:
RAW/PCO roundtrip всех scalar types, mixed/all-null, фиксированные fixtures,
malformed input, CBOR precision/canonicalization и envelope references. Проверены
Clippy и сборка/тесты/package из отдельной директории без workspace приложения.
Полная модель body/graph, редактор, durability, OCR и аппаратная интеграция нового
хранилища ещё требуют реализации и проверок. Бенчмарки другого агента измеряют
кодеки/обрамление; они не доказывают задержку нового storage на BOOX.

## 10. Проверенные источники и следующий порядок работ

Код Tangleaf, проверенный при revision 2:

- src/features/handwriting/ink-model.ts: drawSegment/drawSheet/inkPoint.
- src/features/handwriting/ink-editing.ts: f64 width после scale и interpolated samples.
- src-tauri/src/commands/handwriting.rs и handwriting/storage.rs: IPC validation, SQLite BLOB storage и CAS.
- plugins/tauri-plugin-mobile-system/android/.../OnyxInk.kt: capture normalization.
- crates/notes-blob/src/lib.rs: SHA-256, install_reader, fsync, no-clobber publication.
- crates/notes-core/src/db.rs: WAL+NORMAL; db/attachments.rs: существующие GC roots.

Внешние первичные источники:

- [RFC 8949, deterministic CBOR](https://www.rfc-editor.org/rfc/rfc8949.html#section-4.2.1).
  Правила выбора float и порядка map применены к metadata данного проекта.
- [SQLite synchronous](https://www.sqlite.org/pragma.html#pragma_synchronous).
  WAL+NORMAL сохраняет консистентность, но не обещает durability после потери питания;
  выбран FULL для будущего durable ink ack.
- [pco ChunkConfig](https://docs.rs/pco/1.0.3/pco/struct.ChunkConfig.html):
  compression_level управляет encoder и сам по себе не записывается в stream;
  его поведение может меняться между версиями crate. Для воспроизводимости benchmarks
  фиксируются версия crate и полная конфигурация encoder.
- Локальные REPORT.md и STORAGE-HANDOFF.md в ink-compression-benchmark: результаты
  относятся к указанным там представлениям. Эксперименты results/inkchunk используют
  revision 2 framing и pco 1.0.3; они не заменяют fixtures и device tests реализации Tangleaf.

Порядок реализации: независимый reader/writer+fixtures (первый слой готов) ->
полная модель metadata/body и graph validation -> adapter нового хранилища Tangleaf
с чистым документом без legacy import -> immutable edit model и incremental IPC ->
транзакционный backend/history/GC -> device bench -> подключение ink к обычным
заметкам/sync -> Recognition/поиск. XOPP import/export — отдельный будущий adapter.
Несжатый экспорт с полными колонками сохраняется для исследования. Quantization
не включается выбором PCO-8 и остаётся отдельным будущим решением.
