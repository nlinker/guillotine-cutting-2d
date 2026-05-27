# glas — strip filler + blueprint simplification (Шаг 3)

Шаги 1 и 2 реализованы и закоммичены.
Шаг 3 — два последовательных подшага (A и B), один коммит после обоих.

---

## Контекст

После Шага 2 glas-декодер:
- Использует 4 blueprint: TlH/TlV/BlH/TrV. BlH и TrV добавляют сложность
  (composite box в нижнем/правом углу), но практической пользы не доказано.
- Использует `matrix_pack`, который кладёт только идентичные детали N×M.

**Нужно**:
1. **Подшаг A** — упростить до 2 blueprint (TlH/TlV, composite box всегда в левом
   верхнем углу); поле в Gene — `inverses: SmallVec<[bool; 16]>`, как было до Шага 2.
2. **Подшаг B** — заменить `matrix_pack` на `strip_fill`: bounded knapsack DP,
   который находит сочетание деталей **разных типов**, максимизирующее заполнение
   ширины свободного прямоугольника. Детали в полосе сортируются по высоте убывающей;
   composite box = (cw=сумма ширин, ch=максимальная высота).

---

## Подшаг A: возврат к `inv: bool`

### `src/cut_tree.rs`
- `Blueprint` enum: оставить только `TlH = 0`, `TlV = 1`.
- `Blueprint::N = 2`; `from_u8(v % 2)`.
- Удалить варианты `BlH`, `TrV` из `apply_blueprint`.
- Удалить тесты: `forest_bl_h`, `forest_tr_v`.

### `src/glas/decoder.rs`
- Переименовать тип: `Blueprints = SmallVec<[u8; 16]>` → `Inverses = SmallVec<[bool; 16]>`.
- Поле `Gene::blueprints: Blueprints` → `Gene::inverses: Inverses`.
- В decode-цикле: `let inv = gene.inverses[placed]; let bp = if inv { Blueprint::TlV } else { Blueprint::TlH };`
- Хелпер `gg()`: `inverses: std::iter::repeat(false).take(count).collect()`.
- Обновить тест `snug_rect_preferred_over_selector`:
  `genome[0].blueprints[0] = 1` → `genome[0].inverses[0] = true`.

### `src/glas/ga.rs`
- `random_genome`: `inverses: (0..count).map(|_| rng.next_u64() & 1 != 0).collect()`.
- `mutate`: параметр `blueprint_p: f64` → `inverse_p: f64`;
  логика мутации: `genome[i].inverses[k] = !genome[i].inverses[k]`.
- `run_ga_inner`: передаёт `config.inverse_p` как `inverse_p` (было `blueprint_p`).
- Хелпер `gg()`: `inverses: std::iter::repeat(false).take(count).collect()`.
- Тест `mutate_randomizes_all_blueprints` → `mutate_flips_all_inverses`;
  assert проверяет `inverses` (все значения инвертированы при p=1.0).

---

## Подшаг B: strip_fill

### Алгоритм `strip_fill`

Сигнатура (приватная функция в `src/glas/decoder.rs`):
```rust
fn strip_fill(
    fr_w: u32, fr_h: u32,
    spec: &ProblemSpec,
    next: &[usize],    // next[i] = следующий неразмещённый flat-индекс типа i
    offsets: &[usize], // offsets[i] = начало типа i во flat-массиве
    primary_type_idx: usize,
    prefer_rotate: bool,
    strip_delta: u32,
) -> StripResult
```

**Структуры данных:**
```rust
struct StripItem { type_idx: usize, pw: u32, ph: u32, rotated: bool, count: usize }
struct StripResult { items: Vec<StripItem>, cw: u32, ch: u32 }
```

**Алгоритм:**
1. Для каждого типа i: `remaining = spec.piespecs[i].count - (next[i] - offsets[i])`.
   Пропустить если `remaining == 0`.
   Выбрать ориентацию: `prefer_rotate` для `primary_type_idx`, `false` для остальных.
   Использовать `piece_fits_in(fr_w, fr_h, ...)` → `(pw, ph, rotated)`.
   Пропустить если не влезает по высоте (`ph > fr_h`) или ширине (`pw > fr_w`).
   Добавить в `candidates: Vec<Candidate { type_idx, pw, ph, rotated, remaining }>`.

2. DP (bounded 0/1 knapsack):
   ```
   reachable: Vec<bool> = vec![false; fr_w as usize + 1]; reachable[0] = true
   from: Vec<Option<(usize, u32)>> = vec![None; fr_w as usize + 1]  // (type_idx, pw)
   for c in &candidates:
     for _copy in 0..c.remaining:
       for w in (0..=(fr_w - c.pw) as usize).rev():
         if reachable[w] && !reachable[w + c.pw as usize]:
           reachable[w + c.pw as usize] = true
           from[w + c.pw as usize] = Some((c.type_idx, c.pw))
   ```
   Сложность: O(Σ remaining_i × fr_w). Для fr_w≤3000 и суммарного remaining≤100: ≤300K операций.

3. Найти `best_w`: наибольшее достижимое w в `[fr_w.saturating_sub(strip_delta), fr_w]`.
   Если ничего не найдено — искать от fr_w до 1 (всегда есть, т.к. primary piece влезает).

4. Реконструкция: идти по `from[]` от `best_w` до 0; считать вхождения каждого типа.
   Собрать `StripItem` per type_idx, сортировать по `ph` убывающей.

5. Вернуть `StripResult { items, cw: best_w, ch: max(ph over items) }`.

### Изменения в decode-цикле

Заменить:
```rust
let mp = matrix_pack(fr_w, fr_h, pw, ph, remaining);
let (batch_x, batch_y) = forest.apply_blueprint(leaf_idx, mp.cw, mp.ch, bp);
for row in 0..mp.rows as usize { for col in 0..mp.cols as usize { ... } }
next[gene.type_idx] += mp.n;
```

На:
```rust
let strip = strip_fill(fr_w, fr_h, spec, &next, &offsets,
                       gene.type_idx, gene.rotate, strip_delta);
let (batch_x, batch_y) = forest.apply_blueprint(leaf_idx, strip.cw, strip.ch, bp);
let mut x_cursor = batch_x;
for item in &strip.items {
    for _ in 0..item.count {
        let flat_idx = next[item.type_idx];
        placements.push(Placement {
            sheet_idx,
            piece_idx: flat_idx,
            x: x_cursor,
            y: batch_y,
            rotated: item.rotated,
        });
        x_cursor += item.pw;
        next[item.type_idx] += 1;
    }
}
// next уже обновлён внутри цикла
```

Добавить параметр `strip_delta: u32` к `decode` и `decode_spec`.

### Удалить из `src/glas/decoder.rs`
- `MatrixLayout`, `MatrixPack`, `CandidateIter`
- `ML1`..`ML12`, `LAYOUTS`, `candidate_layouts`, `matrix_pack`
- Тесты: `layout_table_counts_correct`, `large_n_candidate_layouts_are_full_matrices`
- Тест `matrix_2x2_full` → обновить (переименовать + новые позиции)

### Обновление тестов в `src/glas/decoder.rs`

**Тесты, которые остаются без изменений по результату** (логика меняется, но assert'ы те же):
- `two_identical_pieces_form_a_strip`: strip ставит 2×80 = cw=160; TlH: right=(160,0,40,100).
  p0=(0,0), p1=(80,0). ✓
- `strip_overflows_to_next_rect`: fr_w=100, pw=60 → 1 шт в строке.
  batch1: p0=(0,0). Bottom=(0,40,100,60). batch2: p1=(0,40). 3-я деталь → sheet 1. ✓
- `selector_steers_strip_to_different_rect`: проверяет sheet_idx, не позиции. ✓
- `snug_rect_preferred_over_selector`: `genome[0].blueprints[0]=1` → `genome[0].inverses[0]=true`.

**Тест, требующий обновления** (`five_pieces_use_two_batches_when_no_exact_layout_fits`):
- Новое поведение: fr_w=300, pw=100 → strip ставит 3 штуки (cw=300).
  batch1: p0=(0,0), p1=(100,0), p2=(200,0). bottom=(0,100,300,300).
  batch2: strip ставит оставшиеся 2 (cw=200). p3=(0,100), p4=(100,100).
  Обновить assertions и doc-комментарий.

**Тест `matrix_2x2_full` → переименовать в `four_pieces_two_rows`**:
  batch1: p0=(0,0), p1=(100,0). bottom=(0,100,200,100).
  batch2: p2=(0,100), p3=(100,100). Те же assert'ы. ✓

**Новые тесты**:
- `strip_mixes_two_types`: типы A (pw=150,ph=100) и B (pw=100,ph=80), fr_w=350, fr_h=100,
  remaining={A:1,B:2}. Ожидаем: cw=350 (150+100+100) или cw=350 иным способом.
- `strip_respects_height_constraint`: тип C (ph=200) исключён при fr_h=150.
- `strip_uses_primary_type_first`: primary type встречается в результате когда помещается.

### `src/ga.rs` — добавить `strip_delta` в `GaConfig`
```rust
/// Width tolerance for the strip filler: the decoder accepts any fill with
/// total width ≥ fr_w − strip_delta. Value 0 means "take the best achievable fill."
pub strip_delta: u32,
```
`Default::default()`: `strip_delta: 0`.
Обновить `Display::fmt`.

### `src/glas/ga.rs` — пробросить `strip_delta`
- `init_population`: добавить параметр `strip_delta: u32`, передать в `decode`.
- `run_ga_inner`: передать `config.strip_delta` в `init_population` и `decode`.
- `small_config()` в тестах: добавить `strip_delta: 0`.

---

## Верификация

```
cargo test
cargo clippy -- -D warnings
cargo +nightly fmt
cargo run -- calc "2600x1800F:3:400x400/6,495x495/6,270x320/10,150x450/17r"
```
