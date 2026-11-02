# Strategia di visibilità: estrarre i test inline da `src/` → `tests/`

> **Tipo:** decisione di design (spike pilota) · **Data:** 2026-10-07
> **Origine:** rilievo della review esterna 2026-10-07 (code-review task 03+04) —
> debito pre-esistente: 7 moduli `#[cfg(test)]` inline in `src/` che fanno
> sforare il limite di 300 righe/file e violano "test MAI inline in `src/`"
> (`nxm-docs/RULES.md`).
> **Pilota (task 07):** `src/session_reader.rs`. **Da replicare sui file
> rimanenti nei task 08-13:** `registry.rs`, `handoff.rs`, `handler.rs`,
> `sampling.rs`, `audit.rs`, `transport.rs`.

## Contesto e vincoli

- `tests/` compila contro la **lib** e vede **solo gli item `pub`**: da lì non
  si raggiungono né gli item privati né i `pub(crate)`.
- Regole violate insieme: "test mai inline in `src/`" + "≤300 righe/file".
- Verifica di accettazione del brief: `grep -c '#[cfg(test)]' src/<file>.rs`
  deve dare **0** — quindi nessun modulo `#[cfg(test)]` può restare dichiarato
  nel file sorgente, nemmeno incluso da un file esterno via `#[path]`.
- Requisito del brief: nessuna modifica funzionale alla produzione oltre a
  cambi di visibilità **strettamente necessari**, documentati.
- I test spostati devono verificare le stesse cose e restare verdi (skill
  `tdd`: i test verificano comportamento attraverso interfacce pubbliche).

## Opzioni considerate

1. **`pub(crate)` + re-export** — esclusa: `tests/` vede solo l'API `pub`,
   gli item restano irraggiungibili. Rotta per costruzione.
2. **`pub` mirato** (rendere `pub` i soli item toccati dai test, con doc `///`
   + esempio) — funziona, ma allarga la superficie pubblica con helper interni
   che esistono solo per i test. Per i file 08-13 (`section_body`,
   `overlap_score`, `parse`, …) significherebbe pubblicare pezzi di
   implementazione.
3. **Unit test privati in file separato via `#[cfg(test)] #[path] mod tests`**
   — esclusa: il modulo `#[cfg(test)]` resterebbe dichiarato in `src/`,
   violando la lettera della rule ("no `#[cfg(test)]` inline modules") e
   fallendo la verifica di accettazione (`grep` deve dare 0).
4. **Ibrida: tutto in `tests/`, con re-seam dei test accoppiati a item
   privati** — i test che usano solo API `pub` si spostano verbatim; un test
   accoppiato a un item privato si riscrive per osservare **lo stesso
   comportamento** attraverso la più vicina API pubblica, quando quel
   comportamento è lì pienamente osservabile. Fallback documentato: `pub`
   mirato (opzione 2) solo quando il comportamento NON è osservabile via API
   pubblica. **SCELTA.**

## Categorizzazione `nxm-decision` (choice, 2026-10-07)

Output: argmax `3_path_submodule` (P=0.53), `2_targeted_pub` (P=0.46),
`1_pub_crate` (0.01), `4_hybrid_reseam` (0.003) — **confidence 0.07**
(segnale quasi nullo: il backend non vede i vincoli hard del brief). L'output
è advisory ("probabilities + confidence, never thresholds") ed è stato
**respinto** perché le opzioni 1 e 3 sono escluse da vincoli oggettivi
(semantica di `tests/` in Rust; verifica di accettazione `grep == 0`), e tra 2
e 4 il brief ("visibilità strettamente necessaria") e la skill `tdd` (test ai
seam pubblici, mai accoppiati all'implementazione) preferiscono la 4.

## Decisione — il pattern da replicare (task 08-13)

1. **Sposta** l'intero blocco `#[cfg(test)]` in `tests/<module>.rs` come test
   di integrazione contro la lib (`use nxm_session_mcp::<module>::…`).
2. **Test che usano solo item `pub`** → spostati **verbatim** (asserzioni
   identiche). Normalizzazioni consentite, entrambe neutre sul
   comportamento: (a) usare `tempfile::TempDir` al posto di eventuali
   helper `tempdir()` hand-rolled (pattern già usato in
   `tests/logging.rs` / `tests/mcp_stdio.rs`), che elimina anche le
   pulizie manuali `remove_dir_all`; (b) `.expect("contesto")` invece di
   `.unwrap()` sulle sole chiamate di **setup** (stile già usato in
   `tests/logging.rs`; identica semantica di panic, asserzioni invariate).
3. **Test accoppiati a item privati** → per prima cosa cerca il **re-seam**:
   il comportamento è osservabile attraverso un'API `pub` che chiama
   l'item privato? Se sì, riscrivi il test su quel seam (asserendo lo stesso
   comportamento osservabile) e l'item resta privato. Zero visibilità.
4. **Fallback (solo se il re-seam è impossibile** perché il comportamento non
   è raggiungibile dall'esterno, es. edge-case di un parser puro non
   esercitabile via API pubblica): rendere `pub` il singolo item con doc
   `///` + esempio runnable, e documentarlo nel task per-file come
   aggiunta deliberata di superficie.
5. **Mai** `pub(crate)` come strategia per `tests/` (invisibile); **mai**
   `#[path]` submodule (violerebbe la lettera della rule e la grep di
   accettazione).
6. Se un file resta >300 righe **per codice NON-test** dopo l'estrazione,
   segnalarlo come split successivo: fuori dallo scope dell'estrazione.

## Applicazione al pilota `session_reader` (task 07)

- 11 test inline: 10 usavano solo item `pub` (`KiroSessionReader`,
  `resolve_raw`, `ResolveArgs`, `ResolveEnv`) → spostati verbatim in
  `tests/session_reader.rs` (con `TempDir`).
- 1 test (`id_from_session_file_strips_raw_and_ext`) chiamava direttamente
  l'helper **privato** `id_from_session_file` → re-seamed come
  `resolve_derives_id_from_session_file_path`: la derivazione dell'id è
  osservabile al seam pubblico `resolve_raw` (che applica esattamente
  `id_from_session_file` quando `args.id` è `None` e `session_file` è
  impostato). Stesse tre casistiche asserate (`.raw` strippato, prefisso
  `ses_`, passthrough `ses_*`); l'unico ramo non coperto dal test originale
  (stem non-UTF8 → fallback `"session"`) resta non coperto come prima.
- **Zero modifiche al codice di produzione** (nessun cambio di visibilità).
- `src/session_reader.rs`: 458 → 274 righe (< 300). `grep '#[cfg(test)]'` = 0.
- **Conteggio test 254 → 243**: gli inline giravano due volte (target lib +
  target bin, perché sia `lib.rs` che `main.rs` compilano il modulo); ora
  girano una sola volta in `tests/`. Coverage comportamentale invariata.
- Verifica: `cargo build` 0 warning, `cargo clippy --all-targets` 0,
  `cargo test` tutto verde.

## Da fare nei task 08-13 (per ciascun file)

1. Crea branch `work/<nn>-<slug>` da `feature/global-session-registry`.
2. Scrivi `tests/<module>.rs` e verifica che passi **prima** di rimuovere il
   blocco inline (green-before-delete).
3. Rimuovi il blocco `#[cfg(test)]` dal sorgente; ricontrolla build/clippy/test.
4. Applica i punti 2-4 del pattern; ogni eventuale `pub` mirato va motivato
   nel task e nel CHANGELOG.
5. Aggiorna CHANGELOG/CONTEXT; STOP e review esterna prima del merge.
