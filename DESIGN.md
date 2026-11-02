# Session Management MCP Server — Design (`nxm-session-mcp`)

> **Stato:** Draft di design — pre-scaffold.
> **Obiettivo:** Definire un server MCP indipendente dal coding agent che razionalizza e rigenera
> una sessione lunga in un **documento di handoff standard**, così che una nuova sessione riprenda
> il lavoro senza perdere qualità. Include una **fase di audit** del task lungo e un **registry**
> di sessioni con stato consumed/used.
> **Naming:** `nxm-session-mcp` (convenzione `nxm-<dominio>-mcp`, allineato ai pari nel workspace
> `nxm-mcp-servers/`). Parola chiave "session management" per ritrovarlo facilmente.

---

## 1. Problema e razionale

Le sessioni lunghe dei coding agent accumulano rumore, decisioni obsolete, codice scartato e
prompt drift. Oltre una certa frazione di riempimento della finestra di contesto (empiricamente
molto prima del 100% teorico) il modello degrada: aumentano allucinazioni, errori sintattici,
perdita di attenzione ("lost in the middle").

La soluzione: **non un riassunto lossy**, ma una **estrazione strutturata orientata alla ripresa**,
prodotta al momento giusto, con una **revisione critica** dell'ultimo lavoro svolto sotto stress
da contesto, salvata come artefatto versionabile.

### Perché un server MCP (e non una skill)
- **Agnosticismo dal coding agent**: MCP è lo standard aperto supportato da Kiro CLI, Claude,
  Cursor, Codex, Pi, ecc. Un server MCP è un **servizio indipendente**, non legato a un solo harness.
- **Riuso del pattern nxm**: si incastra nel workspace `nxm-mcp-servers/` come pari di
  `nxm-docs-mcp`, `nxm-memory-mcp`, `nxm-gateway-mcp`.

---

## 2. Vincolo architetturale fondamentale (letto dallo standard MCP)

Un server MCP è **reattivo, non proattivo**:
- Espone capacità (Tools / Resources / Prompts); **non gira in background** e **non inizia azioni
  da solo** in base a eventi dell'agente.
- Per **isolamento di contesto by design**, il server **non vede la conversazione** né l'uso reale
  della finestra di contesto. La misura esatta del context% e il trigger automatico appartengono
  al **client/harness** (hook `turn_end` + API tipo `getContextUsage()`), non al server.

Conseguenza: il "sensore che scatta al 60%" **non può vivere dentro il server MCP puro**. Va
separato. Da qui l'architettura a due modalità.

### Feature MCP che usiamo (spec 2025)
| Feature | Ruolo nel nostro server |
|---|---|
| **Tools** | Azioni: generare handoff, audit, gestire registry, marcare consumed. |
| **Resources** | Espone i documenti di handoff in `sessions/` come risorse leggibili (URI + MIME). |
| **Prompts** | Template riutilizzabili (es. "riprendi da handoff", "istruzioni auto-discovery"). |
| **Sampling** | Il server chiede al client di eseguire sintesi/audit **col modello dell'harness** (zero costi extra, privacy: il server non vede la conversazione). |
| **Elicitation** | Notifica e conferma utente ("handoff pronto, apri nuova sessione? [Y/n]"). |
| **Roots** | Confini filesystem → directory `sessions/` nel progetto. |

---

## 3. Architettura: un server, due modalità

Principio chiave: **il trigger è disaccoppiato dal lavoro.** Il server riceve in input (o raccoglie
via Sampling) la conversazione/parametri e il context%, e fa il suo lavoro. *Chi* lo chiama
(umano, agente, hook, watcher) è irrilevante per il server.

### Modalità A — MCP isolato (BASE, deve funzionare SEMPRE)
Server completo e autosufficiente. Nessuna dipendenza dall'harness oltre al protocollo MCP.
Trigger **manuale o auto-dichiarato dal modello**. Tutti i tool utilizzabili a mano.

**Iterazione col modello via Sampling (chiarimento importante):** quando `generate_handoff` viene
invocato, il server **non** richiede tutti i parametri già pronti. Li **estrae iterando col modello
dell'harness via Sampling**: "qual è l'obiettivo corrente? quali decisioni sono state prese? quali
approcci sono falliti? qual è l'ultimo task da revisionare? qual è il tuo context% stimato?".
Questo è **interamente dentro la modalità isolata** — il Sampling è reattivo, parte solo quando il
tool è chiamato. **Non** è la modalità ibrida.

Limite onesto della modalità A: il context% riportato dal modello è **auto-dichiarato e
approssimativo**. La qualità del documento è piena; la precisione del *timing* del trigger no.

### Modalità B — Ibrida (ENHANCEMENT opzionale)
Stesso server + un **adapter di sensing** per-harness (hook `turn_end` o watcher esterno sul file
di sessione) che misura il context% **esatto** e chiama `generate_handoff` **automaticamente** al
superamento soglia, senza intervento umano. Se manca, non si perde nessuna funzionalità del core:
solo l'automazione e la precisione del timing.

### Tabella delle tre precisioni di trigger
| Modalità | Chi misura il context% | Precisione timing | Automazione |
|---|---|---|---|
| Isolata, trigger manuale | l'utente decide | N/A | nessuna |
| Isolata + Sampling | il modello auto-dichiara | approssimativa | nessuna |
| Ibrida (hook/watcher) | l'harness / il file di sessione | esatta | piena |

### Fonte della cronologia in modalità isolata
Tre opzioni, in ordine di preferenza:
1. **Sampling** (preferita): il server chiede all'harness di produrre la sintesi col suo modello e
   contesto, senza mai vedere la conversazione. Più pulita e privacy-safe. Richiede supporto sampling.
2. **Argomento esplicito** (fallback): l'agente passa la cronologia rilevante come argomento del tool.
   Funziona ovunque, consuma token.
3. **Session-reader per-harness** (plugin opzionale): legge il file di sessione dell'harness
   (es. `~/.kiro/sessions/cli/{id}.jsonl` individuato via `$AGENT_SESSION_ID` (fallback `$KIRO_SESSION_ID`)). Zero token, ma
   harness-specifico. Vive nella stessa famiglia degli adapter opzionali.

**Generazione documento:** Sampling come prima scelta, **fallback a modello dedicato configurabile**
(pattern pi-handoff: modello economico tipo Haiku via env). Questo rende il servizio davvero agnostico.

---

## 4. Il documento di Handoff (formato standard)

File Markdown con **frontmatter YAML**. Versionabile con Git. Un esempio:

```markdown
---
schema: nxm-session-handoff/v1
id: ses_20260929_refactor_auth
title: "Refactoring modulo autenticazione JWT"
short_description: "Refactor auth JWT + fix middleware; async token validation"
created: 2026-09-29T09:20:00+02:00
workspace: "~/Projects/app"
origin_session: "f2946a26-3735-4b08-8d05-c928010302d5"  # id sessione harness, se noto
goal_next: "Completare il wiring dell'async token validation nel middleware"
status: unconsumed        # unconsumed | consumed
consumed_at: null
consumed_by: null         # id nuova sessione che l'ha ripreso
context_usage_pct: 62     # stimato o esatto secondo la modalità
trigger: sampling         # manual | sampling | hook | watcher
skills_involved: ["code-review", "security-scan"]  # skill dell'harness rilevanti/usate
---

# Handoff — Refactoring modulo autenticazione JWT

## Suggested first message (paste-ready)
Continue from sessions/ses_20260929_refactor_auth.md.
Goal: completare il wiring dell'async token validation nel middleware.
Leggi il file di handoff per intero prima di iniziare. File chiave: src/auth/middleware.ts.

## Stato attuale (As-Built)
| Area | Stato |
|------|-------|
| COMPLETATO | Interfacce approvate in docs/plan.md; skeleton middleware |
| IN CORSO | Wiring async token validation |
| PROSSIMO | Test di integrazione sul path async |
| BLOCCANTI | nessuno |

## Decisioni (non rimettere in discussione)
- Token → validazione async (il path sync era il collo di bottiglia).

## Audit del task lungo (Sanity check)   ← DIFFERENZIATORE
Revisione critica dell'ultimo output prodotto sotto stress da contesto:
- **Fragilità potenziali**: la gestione errori nel nuovo path async non copre il timeout del provider.
- **Disallineamenti dai requisiti**: nessuno rilevato.
- **Debito tecnico introdotto**: TODO lasciato in middleware.ts:142 (retry non implementato).
- **Verifica suggerita**: eseguire la skill `code-review` sul diff dell'ultimo task prima di proseguire.

## Approcci falliti (da NON ripetere)
- Cache dei token in-process → rotta su deploy multi-worker.

## File rilevanti (ordine di lettura)
1. `docs/plan.md` — piano approvato per fasi
2. `src/auth/middleware.ts` — dove continua il lavoro

## Prossimi passi immediati (todo-list)
1. [ ] Correggere i punti dell'Audit (timeout provider, retry).
2. [ ] Completare wiring async.
3. [ ] Test integrazione.
```

### Note di design del documento
- **Pointers over prose**: riferisci i file per path, non duplicare contenuto recuperabile da Git.
- **Audit in cima ai prossimi passi**: i punti critici emersi dall'audit diventano la prima todo.
- **Privacy**: il documento può contenere dati sensibili → scrittura con permessi `0600`,
  `sessions/` gitignorabile a scelta dell'utente (default: versionato, opt-out disponibile).

---

## 5. Layout `sessions/` e registry

```
<project-root>/
└── sessions/                             # handoff distillati — VERSIONATO in git
    ├── registry.json                     # indice: id, short_description, status, archive_file, timestamps
    ├── active.lock                        # (opzionale) punta alla sessione corrente
    ├── ses_20260929_refactor_auth.md
    ├── ses_20260929_api_payment.md
    └── archive/                          # sessioni RAW complete — GITIGNORED, 0600, TRANSITORIE
        └── ses_20260929_refactor_auth.raw.jsonl
```

> **Due subdirectory, due scopi opposti** (vedi §5bis):
> - `sessions/*.md` = **handoff** distillato, lossy, forward-looking, versionato in git.
> - `sessions/archive/*.raw.jsonl` = **sessione completa** lossless, gitignored, `0600`, **transitoria**
>   (va eliminata dopo essere stata ridotta/usata).

`registry.json` (esempio):
```json
{
  "schema": "nxm-session-registry/v1",
  "sessions": [
    {
      "id": "ses_20260929_refactor_auth",
      "short_description": "Refactor auth JWT + fix middleware",
      "status": "unconsumed",
      "created": "2026-09-29T09:20:00+02:00",
      "consumed_at": null,
      "archive_file": "archive/ses_20260929_refactor_auth.raw.jsonl",
      "file": "ses_20260929_refactor_auth.md"
    }
  ]
}
```

### Gestione stato consumed/used
- Quando un nuovo agente carica un handoff per iniziare, `mark_consumed` imposta
  `status: consumed`, `consumed_at`, `consumed_by`.
- Se l'utente tenta di riprendere una sessione già `consumed`, il server (via Elicitation) avvisa:
  *"Sessione già consumata il <data>, ripresa da <id>. Creare una derivazione o riaprire forzatamente?"*

---

## 5bis. Session Archive: raw completo vs handoff distillato

L'**handoff** (`sessions/*.md`) e l'**archive** (`sessions/archive/*.raw.jsonl`) sono complementari e
servono a scopi opposti:

| | Handoff (`*.md`) | Archive (`*.raw.jsonl`) |
|---|---|---|
| Contenuto | distillato, solo l'essenziale | sessione **completa**, lossless |
| Direzione | forward-looking ("riparti da qui") | backward-looking ("cos'è successo") |
| Scopo | far ripartire un agente | rielaborazione, audit a posteriori, rigenerare un handoff diverso |
| Git | **versionato** | **gitignored** |
| Permessi | normali | `0600` |
| Ciclo di vita | persistente | **transitorio** — eliminato dopo essere stato ridotto/usato |

**Perché serve:** l'handoff è lossy per progetto; se dopo serve qualcosa che è stato distillato via,
si risale all'archive. L'archive permette anche di rigenerare un handoff con un obiettivo diverso senza
rifare la sessione. Il collegamento è nel frontmatter dell'handoff: `source_archive: archive/<id>.raw.jsonl`.

**Confine con `nxm-memory` (deciso):** `nxm-memory` archivia **solo fatti replicabili / di ricerca**,
**mai** una sessione completa. Nessuna sovrapposizione: l'archive di `nxm-session-mcp` è autonomo e
transitorio, non è memoria semantica cross-sessione.

**Fonte del raw (asimmetria rispetto all'handoff):** per l'archive completo la prima scelta **non** è
Sampling (che produce output *sintetizzato*), ma il **session-reader per-harness**: la sessione completa
e lossless **esiste già** su disco (es. Kiro: `~/.kiro/sessions/cli/{id}.jsonl` via `$AGENT_SESSION_ID` (fallback `$KIRO_SESSION_ID`);
Pi/OpenCode nei loro formati). Il server la copia/normalizza in `sessions/archive/`, zero token.
Fallback (modalità isolata pura): l'agente passa la conversazione come argomento del tool.

**Ciclo di vita transitorio:** dopo che un archive è stato usato per produrre/ridurre un handoff, va
eliminato. Un tool dedicato (`purge_archive`) o un flag su `mark_consumed` rimuove il `.raw.jsonl` e
azzera `archive_file` nel registry. Default consigliato: alla `mark_consumed`, proporre via Elicitation
la cancellazione dell'archive collegato.

---

## 6. Tool MCP (interfaccia)

| Tool | Input (sintesi) | Output | Note |
|---|---|---|---|
| `generate_handoff` | `goal_next?`, `context_usage_pct?`, `conversation?` (fallback), `use_sampling?` | handoff `id` + path + draft | Itera col modello via Sampling; poi Elicitation per review. |
| `audit_last_task` | `task_ref?`, `diff?`/`files?` | sezione Audit strutturata | Usa skill `code-review` se presente, altrimenti audit inline via Sampling. |
| `list_sessions` | `status?` (filtro) | elenco da `registry.json` | — |
| `get_session` | `id` | contenuto handoff | Anche esposto come Resource. |
| `find_resumable_session` | `hint?` (task/descrizione) | miglior candidato `unconsumed` | Auto-discovery per il nuovo agente. |
| `mark_consumed` | `id`, `consumed_by?` | ok | Aggiorna registry + frontmatter. Può proporre il purge dell'archive collegato. |
| `archive_session` | `id?`, `conversation?` (fallback), `session_file?` | path `.raw.jsonl` + entry aggiornata | Salva la sessione RAW completa in `sessions/archive/` (0600, gitignored). Prima scelta: session-reader per-harness; fallback: conversazione come argomento. Transitorio. |
| `purge_archive` | `id` | ok | Elimina il `.raw.jsonl` collegato e azzera `archive_file` nel registry. Ciclo di vita transitorio dell'archive. |
| `update_session` | `id`, `goal_next?`, `context_usage_pct?`, `conversation?` (fallback), `use_sampling?` | `id` + path + revision | Upsert-per-id: riscrive `<id>.md` e incrementa `revision` preservando identità e status; mai un nuovo id (via Sampling, come `generate_handoff`). |

### Prompts MCP
- `resume_from_handoff` — istruzione paste-ready per la nuova sessione.
- `autodiscovery_bootstrap` — da mettere nel system prompt del nuovo agente: *"All'avvio, chiama
  `find_resumable_session`; se trovi un handoff `unconsumed` pertinente, leggilo prima di iniziare."*

### Flusso `generate_handoff` (modalità isolata, con Sampling)
1. Agente/utente chiama `generate_handoff`.
2. Server → Sampling: raccoglie goal, decisioni, approcci falliti, ultimo task, context% stimato.
3. Server → `audit_last_task` (Sampling o skill `code-review`) per la sezione Audit.
4. Server assembla il documento + aggiorna `registry.json`.
5. Server → Elicitation: mostra il draft, l'utente può editare/approvare.
6. Su approvazione: scrive `sessions/<id>.md` (perm `0600`), suggerisce `/chat new` + load.

---

## 7. Integrazione skill dell'harness
- L'audit **usa la skill se presente** (es. `code-review`, `security-scan`), altrimenti **fallback**
  ad audit inline via Sampling. Le skill coinvolte vengono registrate nel frontmatter
  (`skills_involved`) per tracciabilità.
- Il rilevamento delle skill disponibili è best-effort e dipende dall'harness; il design non
  assume la loro presenza.

---

## 8. Vendor di riferimento (da aggiungere a VENDORS.md)

Progetti affini da **vendorizzare come reference** (studiare il *pattern d'uso*, non copiare codice;
tutti **MIT**). Da clonare in `/nxm/vendors/` via `clone-vendors.sh`, coerentemente con la policy
"nuovi vendor = clone GitHub diretti".

| Nome | Origin (clone) | Licenza | Cosa copiamo (pattern) |
|---|---|---|---|
| pi-handoff | `github.com/ttiimmaahh/pi-handoff` | MIT | Trigger a soglia (percent/token), Sampling per la sintesi, fallback modello economico, refresh del doc, `0600`, iniezione one-shot su nuova sessione. |
| handoff-skill | `github.com/klittle32/handoff-skill` | MIT | Struttura documento goal-directed: Decisioni / Current state / **Failed approaches** / Relevant files (read order) / Suggested skills; "paste-ready first message"; principi (pointers over prose, disposable, honest). |
| pf-handoff | `github.com/turvodnik/pf-handoff` | MIT | Context-usage sensor, threshold alerts, HANDOFF state files, auto-recovery dopo compaction. |

Riga(he) da aggiungere in `nxm-docs/vendors/VENDORS.md` (categoria "Agent / Runtime" o nuova
"Session / Handoff"):
```
| 23 | pi-handoff     | github.com/ttiimmaahh/pi-handoff   | — | ~1M | Reference handoff MCP/ext: trigger soglia + sampling |
| 24 | handoff-skill  | github.com/klittle32/handoff-skill | — | ~1M | Reference struttura documento handoff goal-directed |
| 25 | pf-handoff     | github.com/turvodnik/pf-handoff    | — | ~1M | Reference context sensor + auto-recovery |
```

---

## 9. Scaffold proposto (allineato a `nxm-mcp-servers/`)

Nuovo membro del workspace Cargo esistente. Struttura coerente con `nxm-docs-mcp`:

```
nxm-mcp-servers/
└── nxm-session-mcp/
    ├── Cargo.toml            # usa workspace.package + workspace.dependencies
    ├── LICENSE-MIT
    ├── LICENSE-APACHE
    ├── README.md
    ├── src/
    │   ├── main.rs           # entrypoint: --transport stdio
    │   ├── protocol.rs       # tipi MCP (riuso pattern esistente)
    │   ├── handler.rs        # dispatch dei tool
    │   ├── handoff.rs        # generazione + schema documento (frontmatter YAML)
    │   ├── audit.rs          # fase di audit del task lungo
    │   ├── registry.rs       # sessions/ + registry.json + stato consumed
    │   └── sampling.rs       # client-side helpers per Sampling/Elicitation
    └── tests/
        └── mcp_stdio.rs      # test end-to-end via stdio (pattern esistente)
```

Aggiornare `nxm-mcp-servers/Cargo.toml`:
```toml
members = [
    "nxm-docs-mcp",
    "nxm-gateway-mcp",
    "nxm-memory-mcp",
    "nxm-webtomd",
    "nxm-session-mcp",   # nuovo
]
```

Config `mcp.json` (coerente con l'esistente, trasporto stdio):
```json
{
  "mcpServers": {
    "nxm-session": {
      "command": "nxm-session-mcp",
      "args": ["--transport", "stdio"],
      "env": { "RUST_LOG": "info" }
    }
  }
}
```

**Split public/private**: **DECISO — fully public MIT/Apache** (come `nxm-docs-mcp`). Ogni server
ha `LICENSE-MIT` + `LICENSE-APACHE`.

---

## 10. Configurazione
| Chiave / env | Default | Significato |
|---|---|---|
| `NXM_SESSION_THRESHOLD` | `60%` | Soglia context (percent o token assoluti). |
| `NXM_SESSION_MODEL` | `auto` | Modello per la generazione se Sampling non disponibile. |
| `NXM_SESSION_PATH` | `sessions/` | Directory dei documenti. |
| `NXM_SESSION_GIT` | `versioned` | `versioned` oppure `gitignored`. |

Sulla **soglia**: default 60% (max sensato ~50–60%; il 30% iniziale è troppo conservativo e
genererebbe handoff troppo frequenti). Da **validare empiricamente** perché il degrado dipende dal
modello. Regola invariante: **mai troncare un task**; scatta al **primo task completato dopo il
superamento soglia**.

---

## 11. Piano d'azione (verso lo scaffold)

1. **Confermare le decisioni aperte** (§13): split public/private, `sessions/` versionato vs
   gitignored, primo harness target per la modalità B.
2. **Vendorizzare i 3 reference** in `/nxm/vendors/` e aggiornare `VENDORS.md`. Studiare:
   schema documento (handoff-skill), trigger+sampling (pi-handoff), sensor (pf-handoff).
3. **Scaffold `nxm-session-mcp`** nel workspace (Cargo member + struttura src + LICENSE + test stub).
4. **Core modalità A**: implementare i 6 tool + Resources + Prompts, con Sampling e fallback
   argomento esplicito. Registry + stato consumed/used.
5. **Fase di audit** (`audit.rs`): audit inline via Sampling + hook per skill `code-review`.
6. **Test stdio end-to-end** (pattern `mcp_stdio.rs`): generate → registry → mark_consumed →
   find_resumable_session.
7. **Adapter modalità B** (separato, opzionale): iniziare dall'harness target scelto (hook
   `turn_end` o watcher esterno sul file di sessione).
8. **Validazione empirica soglia**: misurare dove inizia il degrado sui modelli usati; tarare default.

---

## 12. Rischi e mitigazioni
| Rischio | Mitigazione |
|---|---|
| Il client non supporta Sampling | Fallback: argomento esplicito + modello dedicato configurabile. |
| Audit poco accurato | Usare skill `code-review` quando presente; marcare gli alert come "da verificare". |
| Context% impreciso (modalità A) | Documentato come limite; modalità B per precisione. |
| Handoff troppo frequenti | Soglia 60% + regola "solo a task completato" + configurabilità. |
| Dati sensibili nel documento | Permessi `0600`, opzione gitignore, nessun echo di segreti. |

---

## 13. Decisioni aperte (da confermare con l'utente)
1. ~~**Split public/private** di `nxm-session-mcp`~~ → **DECISO (2026-09-29): fully public MIT/Apache.**
   È infrastruttura di produttività, non IP di inferenza (diverso da `nxm-memory`).
2. ~~**`sessions/` versionato in Git o gitignored di default?**~~ → **DECISO (2026-09-29):
   versionato in Git di default**, con opt-out via `NXM_SESSION_GIT=gitignored`.
3. ~~**Primo harness target** per la modalità ibrida~~ → **DECISO (2026-09-29): Pi, OpenCode,
   Kiro CLI.** Ordine di reference: Pi (pi-handoff è il più diretto: hook `turn_end` +
   `getContextUsage()`), OpenCode, Kiro CLI (hook + `$AGENT_SESSION_ID` (legacy `$KIRO_SESSION_ID`), session-reader su
   `~/.kiro/sessions/cli/{id}.jsonl`).
4. ~~**Collocazione del design**~~ → **DECISO (2026-09-29): sì, copia anche dentro
   `nxm-mcp-servers/nxm-session-mcp/`** (oltre a `nxm-docs/design/active/`).

---

## Appendice — Fonti consultate (2026-09-29)
- MCP feature guide (Tools/Resources/Prompts/Sampling/Roots/Elicitation) — workos.com.
- MCP spec: server tools, lifecycle — modelcontextprotocol.io.
- Isolamento di contesto / limiti payload MCP — vari (apideck, roadie, microsoft mcscatblog).
- Reference handoff: pi-handoff, handoff-skill, pf-handoff (README, tutti MIT).
- Kiro CLI: session management, `/compact`, `/rewind`, hook + `$AGENT_SESSION_ID` (legacy `$KIRO_SESSION_ID`) (docs introspette).
