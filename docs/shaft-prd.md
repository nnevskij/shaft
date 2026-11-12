# Product Requirements Document: Shaft — client ngrok per la menu bar di macOS

**Versione**: 1.0
**Data**: 2026-11-12
**Autore**: Sarah (Product Owner)
**Quality Score**: 92/100
**Stato**: Bozza per avvio sviluppo

> Nome del progetto: **Shaft** (il pozzo, o la galleria verticale, che collega il locale alla superficie). Il nome non usa "ngrok" né il suo logo, perché è un marchio registrato di ngrok Inc. (vedi Risk Assessment). Disponibilità del nome su GitHub, crates.io e Homebrew ancora da verificare.

---

## Executive Summary

Shaft è un'applicazione macOS open source, scritta interamente in Rust, che vive esclusivamente nella menu bar (nessuna icona nel Dock, nessuna finestra principale) e funge da client grafico completo per ngrok. Permette di avviare, monitorare e fermare tunnel HTTP/TCP/TLS, ispezionare e ri-eseguire le richieste in arrivo, gestire la configurazione e più account, e amministrare le risorse cloud dell'account ngrok (domini, endpoint, indirizzi TCP, credenziali, traffic policy) — tutto ciò che oggi richiede la CLI `ngrok` e la dashboard web.

Il target sono sviluppatori che usano ngrok quotidianamente e oggi sono costretti a tenere aperti terminali dedicati, ricordare flag e comandi, e passare al browser per vedere il traffico o gestire i domini. Shaft porta tutto a due clic di distanza, con un'interfaccia AppKit nativa (popover + finestre on-demand), notifiche e scorciatoie globali.

L'architettura non dipende dal binario `ngrok`: i tunnel girano in-process tramite l'SDK Rust ufficiale (crate `ngrok`), mentre le risorse dell'account sono gestite via API REST (`api.ngrok.com`). Questo dà un'app autosufficiente, leggera e con pieno controllo sul traffico — il che rende possibile un ispettore richieste integrato.

---

## Problem Statement

**Situazione attuale**
- Ogni tunnel richiede un terminale aperto (`ngrok http 3000`); chiudere la finestra chiude il tunnel.
- Avviare tunnel ricorrenti richiede di ricordare flag (`--url`, `--basic-auth`, `--traffic-policy-file`, …) o di mantenere a mano `ngrok.yml`.
- Non c'è una vista d'insieme di cosa è esposto in un dato momento.
- L'ispettore web (`localhost:4040`) esiste solo finché l'agente CLI è attivo ed è separato dal resto.
- Gestire domini riservati, endpoint, credenziali e policy richiede la dashboard web o comandi `ngrok api …` verbosi.
- Passare tra account (personale/lavoro) significa modificare file di configurazione.

**Soluzione proposta**
Un'app menu-bar nativa che unifica: gestione tunnel con profili salvati, ispettore richieste integrato, gestione config e multi-account con segreti nel Portachiavi, e un pannello di amministrazione delle risorse API.

**Impatto atteso**
- Avvio di un tunnel salvato + copia URL in ≤ 2 clic (vs. aprire terminale, digitare comando, copiare URL).
- Zero terminali dedicati a ngrok.
- Visibilità costante dello stato dei tunnel dall'icona in menu bar.

---

## Success Metrics

**KPI primari**

| # | Metrica | Target | Metodo di misura |
|---|---------|--------|------------------|
| 1 | Parità funzionale con la CLI | 100% dei comandi rilevanti della matrice di parità (Appendice A) coperti in v1.0 | Checklist di parità verificata prima del rilascio |
| 2 | Sostituzione d'uso | L'autore/maintainer non usa CLI né dashboard per 2 settimane consecutive di lavoro quotidiano | Dogfooding, diario d'uso |
| 3 | Avvio app (cold start → icona visibile) | < 1 s | Misura con `os_signpost` / log temporizzati su M1 base |
| 4 | Memoria a riposo (0 tunnel, popover chiuso) | < 50 MB RSS | Activity Monitor / `footprint` |
| 5 | Tempo clic → tunnel online (URL disponibile) | < 3 s p95 su rete normale | Telemetria locale nel log di debug (nessun invio remoto) |
| 6 | Stabilità | 0 crash noti in v1.0; riconnessione automatica dopo sleep/wake e cambio rete entro 10 s | Test manuali scriptati + crash report locali |
| 7 | Adozione open source | 6 mesi dopo v1.0: ≥ 500 stelle GitHub, ≥ 1.000 installazioni Homebrew, ≥ 5 contributor esterni | GitHub insights, analytics Homebrew |

**Validazione**: KPI 1–6 verificati come gate di rilascio della v1.0; KPI 7 rivisto a 3 e 6 mesi dal rilascio.

---

## User Personas

### Primaria: Marco, sviluppatore full-stack
- **Ruolo**: sviluppatore web/mobile, usa ngrok più volte al giorno (webhook Stripe/GitHub, demo a clienti, test su device mobili).
- **Obiettivi**: esporre un servizio locale in un attimo con un dominio stabile; vedere cosa arriva dai webhook; ri-eseguire una richiesta senza rifare l'azione esterna.
- **Pain point**: terminali dimenticati aperti, flag da ricordare, alt-tab continui verso `localhost:4040` e la dashboard.
- **Livello tecnico**: avanzato.

### Secondaria: Giulia, tech lead / freelance multi-cliente
- **Ruolo**: gestisce più account ngrok (personale + aziendale), domini riservati, policy di accesso (OAuth, IP restriction).
- **Obiettivi**: passare da un account all'altro senza toccare file; amministrare domini, endpoint e credenziali senza browser.
- **Pain point**: config sparse, segreti in chiaro in `ngrok.yml`, dashboard web lenta per operazioni rapide.
- **Livello tecnico**: avanzato.

---

## User Stories & Acceptance Criteria

### Story 1: Primo avvio e onboarding

**As a** nuovo utente
**I want to** collegare il mio account ngrok in pochi secondi
**So that** posso iniziare subito a creare tunnel

**Acceptance Criteria:**
- [ ] Al primo avvio compare solo l'icona in menu bar (nessuna icona Dock: `LSUIElement = true`) e si apre automaticamente il popover di onboarding.
- [ ] L'utente può incollare un authtoken oppure importare un `ngrok.yml` esistente (percorso di default `~/Library/Application Support/ngrok/ngrok.yml`, rilevato automaticamente).
- [ ] L'API key è opzionale nell'onboarding; le funzioni "Risorse API" mostrano un invito a configurarla se assente.
- [ ] L'authtoken viene validato aprendo una sessione di prova; in caso di errore viene mostrato il messaggio/codice d'errore ngrok (es. `ERR_NGROK_105`) con link alla documentazione.
- [ ] I segreti vengono salvati esclusivamente nel Portachiavi di macOS, mai su disco in chiaro.

### Story 2: Avviare un tunnel rapido

**As a** Marco
**I want to** esporre `localhost:3000` con un clic
**So that** posso condividere subito il servizio

**Acceptance Criteria:**
- [ ] Dal popover: campo "Quick tunnel" con protocollo (HTTP/TCP/TLS) e upstream (porta, `host:port` o URL, incl. `https://` e unix socket per HTTP).
- [ ] Il tunnel è online in < 3 s p95; l'URL pubblico viene mostrato e (opzione attiva di default) copiato negli appunti con notifica.
- [ ] L'icona in menu bar riflette lo stato aggregato: inattivo / ≥1 tunnel online / errore o riconnessione.
- [ ] Se l'upstream locale non risponde, il tunnel resta online ma la riga mostra un avviso "upstream irraggiungibile" e l'ispettore registra il 502.
- [ ] Se il limite di sessioni/endpoint del piano è raggiunto, viene mostrato l'errore ngrok con suggerimento.

### Story 3: Profili di tunnel salvati

**As a** Marco
**I want to** salvare configurazioni di tunnel ricorrenti con tutte le opzioni
**So that** non devo ricordare flag e posso avviarle in 2 clic

**Acceptance Criteria:**
- [ ] Un profilo contiene: nome, protocollo, upstream, URL/dominio (incl. domini riservati e `*.internal`), metadata, binding (public/internal/kubernetes ove supportato dall'SDK), pooling, traffic policy (inline YAML o file), opzioni specifiche di protocollo (vedi Feature 2).
- [ ] I profili sono importabili da ed esportabili verso `ngrok.yml` (sezione `endpoints` del formato v3; lettura anche del formato legacy v2 `tunnels`).
- [ ] Ogni profilo ha un flag "preferito" (fissato in cima al popover e assegnabile a scorciatoia globale).
- [ ] Avvio multiplo: "Avvia tutti i preferiti" e selezione multipla.
- [ ] Validazione prima del salvataggio (porta valida, YAML della policy sintatticamente corretto, dominio appartenente all'account se l'API key è configurata).

### Story 4: Ispezionare e ri-eseguire richieste

**As a** Marco
**I want to** vedere in tempo reale le richieste HTTP che arrivano al tunnel e poterle ri-eseguire
**So that** posso fare debug dei webhook senza rigenerarli

**Acceptance Criteria:**
- [ ] Finestra "Ispettore" (NSWindow on-demand) con lista richieste per tunnel: metodo, path, status, durata, dimensione, timestamp; aggiornamento live.
- [ ] Dettaglio: header e body di request e response con visualizzazione raw, JSON formattato, form-urlencoded e hex per binari; body compressi (gzip/br/deflate) decodificati.
- [ ] Replay: ri-invio identico all'upstream locale; "Replay con modifiche" consente di modificare metodo, path, header e body.
- [ ] Filtri per testo, metodo, range di status; pulizia della lista; copia come `curl`.
- [ ] Buffer in memoria limitato e configurabile (default: ultime 500 richieste per tunnel, body troncati oltre 1 MB con indicazione).
- [ ] Le richieste WebSocket/upgrade sono elencate (handshake) ma i frame non sono catturati in v1.0.
- [ ] L'ispezione può essere disattivata per profilo (traffico inoltrato senza cattura, overhead minimo).

### Story 5: Multi-account

**As a** Giulia
**I want to** gestire più account ngrok e passare dall'uno all'altro
**So that** separo progetti personali e di lavoro

**Acceptance Criteria:**
- [ ] Gestione account: nome, colore/etichetta, authtoken, API key opzionale, region/server_addr opzionale, proxy opzionale.
- [ ] Ogni profilo di tunnel è associato a un account; tunnel di account diversi possono essere attivi contemporaneamente (una sessione SDK per account).
- [ ] L'account attivo per le "Risorse API" si seleziona dal popover/finestra.
- [ ] La rimozione di un account chiede conferma, ferma i suoi tunnel ed elimina i relativi segreti dal Portachiavi.

### Story 6: Amministrare le risorse dell'account

**As a** Giulia
**I want to** gestire domini riservati, endpoint, indirizzi TCP, credenziali e policy dall'app
**So that** non devo aprire la dashboard web

**Acceptance Criteria:**
- [ ] Finestra "Account" con sezioni per risorsa (vedi Feature 5): lista, dettaglio, creazione, modifica, eliminazione con conferma.
- [ ] Le azioni distruttive richiedono conferma esplicita con il nome della risorsa.
- [ ] Errori API (401/403/404/429/5xx) mostrati con messaggio leggibile; 429 gestito con backoff e retry.
- [ ] Le funzioni non disponibili nel piano dell'utente mostrano l'errore restituito dall'API senza bloccare il resto dell'app.
- [ ] Da un dominio riservato si può creare direttamente un profilo di tunnel che lo usa.

### Story 7: Notifiche e scorciatoie

**As a** utente
**I want to** essere avvisato dei cambi di stato e controllare l'app da tastiera
**So that** non devo continuamente aprire il popover

**Acceptance Criteria:**
- [ ] Notifiche (UNUserNotificationCenter) per: tunnel online (con URL), tunnel caduto, riconnesso, errore di autenticazione/limite piano. Ogni categoria è attivabile o disattivabile.
- [ ] Scorciatoie globali configurabili per: apri/chiudi popover, avvia/ferma ciascun preferito, copia URL dell'ultimo tunnel avviato. Funzionano senza permessi di Accessibilità (registrazione via Carbon `RegisterEventHotKey`).
- [ ] Rilevamento conflitti di scorciatoia con messaggio all'utente.

### Story 8: Resilienza

**As a** utente
**I want to** che i tunnel sopravvivano a sleep, cambi di rete e brevi disconnessioni
**So that** posso fidarmi dell'app

**Acceptance Criteria:**
- [ ] Dopo wake da sleep o cambio di rete (Wi-Fi ↔ Ethernet ↔ hotspot) i tunnel attivi si ricollegano automaticamente entro 10 s dal ripristino della connettività.
- [ ] Durante la riconnessione lo stato è "Riconnessione…" con backoff esponenziale (max 60 s); dopo N tentativi falliti (default 10) il tunnel va in errore con notifica.
- [ ] Alla chiusura dell'app tutti i tunnel vengono chiusi in modo pulito (sessioni chiuse lato ngrok).
- [ ] Un panic in un task di tunnel non fa crashare l'app: il tunnel va in errore e il resto continua.

---

## Functional Requirements

### Feature 1: Menu bar e popover
- **Descrizione**: `NSStatusItem` con icona template (monocromatica, adattiva light/dark) che riflette lo stato aggregato; clic sinistro apre un `NSPopover` (con vibrancy), clic destro un menu nativo minimale (Impostazioni, Info, Esci).
- **Contenuto del popover**: selettore account; quick tunnel; lista tunnel attivi (nome, URL con pulsante copia, stato, contatore richieste, pulsante stop, apri nell'ispettore, apri nel browser); lista profili/preferiti con pulsante avvia; link a Ispettore, Account, Impostazioni.
- **Edge case**: menu bar affollata/notch che nasconde l'icona → hotkey globale per aprire il popover; popover chiuso al clic esterno (comportamento `transient`).
- **Error handling**: errori mostrati inline sulla riga del tunnel, con dettaglio espandibile e codice `ERR_NGROK_*`.

### Feature 2: Gestione tunnel (parità con `ngrok http|tcp|tls|start`)
- **Protocolli**: HTTP/HTTPS, TCP, TLS.
- **Opzioni comuni**: URL/dominio, metadata, binding, pooling, description, traffic policy (inline o file), forward verso `host:port`/URL, proxy protocol verso upstream ove supportato.
- **Opzioni HTTP**: basic auth, OAuth (provider + filtri email/domini), OIDC, webhook verification, IP restrictions (CIDR allow/deny), header request/response add/remove, host-header rewrite, compressione, circuit breaker, mutual TLS CA, upstream HTTP/2 e verifica TLS upstream, scheme. Dove ngrok ha spostato queste funzioni su Traffic Policy, l'app genera la policy equivalente invece di usare opzioni deprecate.
- **Opzioni TCP**: indirizzo riservato (remote addr), IP restrictions, policy.
- **Opzioni TLS**: dominio, terminazione TLS all'edge con certificato/chiave o pass-through, mutual TLS, policy.
- **Flusso**: utente sceglie profilo o compila quick tunnel → core crea il listener SDK sulla sessione dell'account → forwarding verso upstream (con eventuale cattura per l'ispettore) → UI aggiornata via evento.
- **Edge case**: dominio già in uso da un'altra sessione (errore ngrok specifico, offrire pooling se applicabile); porta upstream occupata/chiusa; dominio non posseduto.
- **Editor policy**: editor YAML con evidenziazione sintassi base e validazione strutturale lato client; validazione definitiva tramite risposta dell'SDK/API.

### Feature 3: Ispettore richieste
- **Descrizione**: per i tunnel HTTP, invece di `listen_and_forward` opaco, il core accetta le connessioni dal listener SDK e le serve con un reverse proxy `hyper` verso l'upstream, registrando request/response in un ring buffer.
- **Requisiti**: supporto HTTP/1.1 e HTTP/2 dall'edge; streaming del body senza bufferizzare oltre il limite di cattura; upgrade WebSocket passati in modo trasparente.
- **Replay**: ricostruisce la richiesta catturata e la invia direttamente all'upstream (non passa dall'edge ngrok); la richiesta replay appare in lista marcata come tale.
- **Export**: copia come `curl`, export HAR della sessione (nice-to-have v1.0, vedi Priorità).
- **Edge case**: body enormi (troncamento), body binari (hex), chunked/streaming lunghi (SSE: cattura dei primi N KB), richieste in corso al momento dello stop del tunnel.

### Feature 4: Configurazione e account (parità con `ngrok config …`)
- `add-authtoken` / `add-api-key` → gestione account con segreti nel Portachiavi.
- `check` → validazione dei profili e del file importato.
- `edit` → editor dei profili nell'app + "Apri ngrok.yml nell'editor esterno".
- `upgrade` → import di config v2 con conversione al modello interno (ed export v3).
- **Archiviazione**: impostazioni e profili in `~/Library/Application Support/<bundle-id>/config.toml` (o JSON), versionati con migrazione di schema; nessun segreto nel file.
- **Interoperabilità**: import/export bidirezionale con `ngrok.yml`; l'app non modifica mai il file della CLI senza azione esplicita dell'utente.
- **Impostazioni globali**: copia URL automatica, categorie di notifiche, scorciatoie, limiti buffer ispettore, livello di log, apertura log, server_addr/region/proxy per account.

### Feature 5: Risorse API (parità con `ngrok api …`)
- **Client**: client REST tipizzato (`reqwest` + `serde`) per `https://api.ngrok.com` con header `Authorization: Bearer <API key>` e `Ngrok-Version: 2`; paginazione gestita; backoff su 429.
- **Risorse incluse in v1.0**:
  - Reserved Domains (create/list/update/delete, certificati gestiti/custom, CNAME target per domini custom)
  - Reserved Addresses TCP
  - Endpoints (lista endpoint online e cloud endpoint con relativa traffic policy: create/update/delete)
  - Tunnel sessions / agenti online (lista, dettaglio, restart/stop/update ove consentito dall'API)
  - Credentials (authtoken), API Keys, SSH credentials
  - IP Policies e IP Policy Rules, IP Restrictions
  - TLS Certificates
  - Event subscriptions/destinations (lista e CRUD base)
- **Risorse non incluse in v1.0**: vedi Out of Scope; un pannello "Explorer API" generico (metodo + path + JSON) copre comunque qualsiasi endpoint non modellato, garantendo la parità con `ngrok api`.
- **Edge case**: API key con permessi ristretti (403 per sezione, le altre continuano a funzionare); risorse create da altri membri del team; liste molto lunghe (paginazione + ricerca).

### Feature 6: Notifiche, scorciatoie, integrazione di sistema
- Notifiche via `UNUserNotificationCenter` (richiede bundle firmato); richiesta permesso alla prima occorrenza, non all'avvio.
- Scorciatoie globali via Carbon `RegisterEventHotKey` (nessun permesso di Accessibilità).
- Osservazione sleep/wake (`NSWorkspace` notifications) e rete (`NWPathMonitor`) per riconnessione proattiva.
- Single instance: un secondo avvio porta in primo piano il popover dell'istanza esistente.

### Feature 7: Diagnostica (parità con `ngrok diagnose`, `ngrok version`)
- Finestra "Diagnostica": test di connettività verso l'edge ngrok (DNS, TCP/TLS, latenza per region), verifica authtoken, verifica API key, versione app e SDK.
- Log strutturati (`tracing`) su file con rotazione in `~/Library/Logs/<bundle-id>/`; "Copia report diagnostico" senza segreti.

### Out of Scope (v1.0)
- Windows e Linux.
- Distribuzione su Mac App Store (sandbox incompatibile con alcune scelte, es. hotkey/rete senza restrizioni).
- `ngrok service` (installazione come servizio di sistema), `ngrok update`, `ngrok completion`: non applicabili a un'app GUI.
- Avvio al login e auto-start dei tunnel (esplicitamente escluso dalle preferenze dell'utente; candidato per Phase 2).
- Companion CLI e URL scheme (Phase 2).
- Cattura dei frame WebSocket e dei payload TCP/TLS raw nell'ispettore.
- Risorse API legacy/deprecate (Edges HTTPS/TCP/TLS e relativi moduli, Tunnel group backends) e funzioni enterprise di nicchia (Bot users, Application users/sessions, Kubernetes operator, Secrets/Vaults) — accessibili solo tramite l'Explorer API generico.
- Telemetria remota di qualsiasi tipo.
- Localizzazione: v1.0 in inglese con infrastruttura i18n pronta; italiano come prima traduzione in Phase 2.

---

## Technical Constraints

### Performance
- Cold start < 1 s fino a icona visibile; popover aperto in < 150 ms.
- RSS a riposo < 50 MB; con 5 tunnel attivi e ispettore pieno (5×500 richieste) < 200 MB.
- Overhead di latenza dell'ispettore < 5 ms p95 per richiesta rispetto al forwarding diretto.
- CPU a riposo ~0% (nessun polling: aggiornamenti event-driven; polling API solo con finestra Account aperta).

### Security
- Authtoken e API key solo nel Portachiavi (`security-framework`), un item per account; mai nei log (redaction obbligatoria nei `Debug`/`tracing`).
- Hardened Runtime, firma Developer ID, notarizzazione Apple per ogni release.
- Aggiornamenti firmati (Sparkle 2 con firma EdDSA) e feed servito su HTTPS.
- Il buffer dell'ispettore può contenere dati sensibili: solo in memoria, mai persistito su disco salvo export esplicito; svuotato allo stop del tunnel (configurabile).
- Nessuna telemetria; nessuna comunicazione verso host diversi da ngrok (edge + API) e dal feed degli aggiornamenti.
- Dipendenze verificate con `cargo audit` e `cargo deny` in CI.

### Integration
- **ngrok Rust SDK (crate `ngrok`)**: sessioni, listener/forwarder, opzioni endpoint, traffic policy. Versione pinnata; aggiornamenti seguiti con test di regressione.
- **ngrok REST API v2**: risorse account (Feature 5).
- **ngrok.yml**: import/export formato v3 (`endpoints`) e lettura v2 (`tunnels`).
- **macOS**: AppKit (NSStatusItem, NSPopover, NSWindow, NSPasteboard), UserNotifications, Network (NWPathMonitor), Carbon HotKeys, Security (Keychain).

### Technology Stack
- **Linguaggio**: Rust stable (edition 2021+), MSRV dichiarata in `Cargo.toml`.
- **UI**: AppKit nativo tramite `objc2` + `objc2-app-kit` / `objc2-foundation` (e crate `objc2-*` per gli altri framework). Nessuna webview. Le viste complesse (tabella ispettore, editor) usano `NSTableView`/`NSOutlineView`/`NSTextView`.
- **Async/rete**: `tokio`, `hyper`/`hyper-util` (reverse proxy ispettore), `reqwest` (API).
- **Serializzazione**: `serde`, `serde_yaml` (ngrok.yml, policy), `toml`/`serde_json` (config app).
- **Logging**: `tracing` + `tracing-appender`.
- **Threading**: AppKit esclusivamente sul main thread; runtime tokio su thread di background; comunicazione core → UI tramite canali e dispatch sul main queue (GCD via `dispatch2`); UI → core tramite comandi (`mpsc`).
- **Struttura workspace (proposta)**:
  - `core` — modello dominio, tunnel manager, stato, eventi (nessuna dipendenza AppKit, testabile)
  - `ngrok-api` — client REST tipizzato
  - `inspector` — reverse proxy + ring buffer + replay
  - `config` — persistenza, migrazioni, import/export ngrok.yml
  - `platform-macos` — Keychain, notifiche, hotkey, sleep/rete
  - `app` — binario AppKit, bundle `.app`
- **Build e packaging**: `cargo-bundle` o script dedicato per `.app` (Info.plist con `LSUIElement=true`, `LSMinimumSystemVersion=13.0`); universal binary (arm64 + x86_64); DMG + Homebrew cask.
- **CI**: GitHub Actions su runner macOS — `cargo fmt`, `clippy -D warnings`, test, `cargo deny`, build universal, firma/notarizzazione sui tag.
- **Compatibilità**: macOS 13 Ventura o successivo, Apple Silicon e Intel.

---

## MVP Scope & Phasing

Su richiesta, tutte le aree confluiscono in un'unica release pubblica **v1.0**. Le milestone sotto sono interne (ordine di sviluppo) e non sono rilasci pubblici.

### Phase 1: v1.0 (rilascio pubblico unico)

| Milestone interna | Contenuto | Priorità |
|---|---|---|
| M0 — Fondamenta | Workspace, shell AppKit (status item, popover, finestre), bridge tokio↔main thread, logging, CI, bundle firmato | P0 |
| M1 — Tunnel core | Sessioni SDK, quick tunnel HTTP/TCP/TLS, stato live, copia URL, resilienza (sleep/rete/backoff) | P0 |
| M2 — Config e account | Multi-account, Portachiavi, profili, import/export ngrok.yml, impostazioni | P0 |
| M3 — Ispettore | Reverse proxy, ring buffer, finestra ispettore, replay, copia curl | P0 (export HAR: P2) |
| M4 — Risorse API | Client REST, Domini, Indirizzi, Endpoint, Credenziali, API key, IP policy, Certificati, Explorer API generico | P0 (Event subscriptions, SSH credentials: P1) |
| M5 — Rifinitura | Notifiche, hotkey, diagnostica, opzioni avanzate di protocollo, accessibilità VoiceOver, Sparkle, Homebrew cask, documentazione | P0/P1 |

**Definizione di MVP (v1.0)**: un utente può installare l'app, collegare uno o più account, avviare/fermare qualunque tunnel esprimibile con la CLI, ispezionare/ri-eseguire il traffico HTTP e gestire le principali risorse dell'account, senza mai aprire CLI o dashboard (gate: matrice di parità all'Appendice A verde e KPI 1–6 rispettati).

### Phase 2: miglioramenti post-lancio
- Avvio al login (`SMAppService`) e auto-start dei profili.
- Companion CLI (`shaft start <profilo>`) e URL scheme `shaft://`.
- Localizzazione italiana.
- Export HAR e persistenza opzionale delle sessioni dell'ispettore.
- Statistiche di traffico per tunnel (grafici richieste/latency).

### Future Considerations
- Cattura frame WebSocket.
- Widget macOS / Shortcuts (App Intents) per avviare profili.
- Condivisione profili tra team (file firmati).
- Visualizzatore/editor grafico delle Traffic Policy.

---

## Risk Assessment

| Rischio | Probabilità | Impatto | Mitigazione |
|---|---|---|---|
| L'SDK Rust non espone tutte le opzioni della CLI (o le depreca a favore di Traffic Policy) | Media | Alto | Matrice di parità verificata con uno spike in M0; dove un'opzione manca, generare la Traffic Policy equivalente; segnalare gap upstream (SDK open source) |
| Breaking change nell'SDK o nell'API ngrok | Media | Medio | Versione pinnata, test di integrazione contro account di test in CI (nightly), changelog monitorato |
| Complessità e `unsafe` di AppKit via objc2 (memoria, thread, delegate) | Alta | Medio | Strato `platform`/`ui` sottile e isolato; tutta la logica in crate puri e testati; regole rigide main-thread; spike iniziale su NSPopover + NSTableView |
| Ispettore introduce latenza o bug di protocollo (HTTP/2, upgrade, streaming) | Media | Alto | Usare `hyper` maturo; possibilità di disattivare la cattura per profilo; suite di test con casi limite (SSE, WebSocket, chunked, body grandi) |
| Uso del marchio "ngrok" | Media | Medio | Nome (Shaft) e icona indipendenti; dicitura "unofficial client for ngrok" nel README; nessun logo ngrok |
| Rate limit API (429) con finestra Account aperta | Bassa | Basso | Cache locale, refresh manuale + polling lento solo a finestra visibile, backoff |
| Notarizzazione/firma richiede account Apple Developer a pagamento | Alta | Medio | Prevedere account Developer del maintainer; fallback: build non firmata per sviluppatori, con istruzioni |
| Esposizione di dati sensibili nel buffer dell'ispettore | Bassa | Alto | Solo in memoria, limiti di dimensione, svuotamento allo stop, nessuna persistenza implicita |
| Adozione OSS bassa (concorrenza di altri client) | Media | Basso | README curato con GIF, Homebrew cask, release frequenti, issue "good first issue" |

---

## Dependencies & Blockers

**Dipendenze**
- Account ngrok di test (free + uno a pagamento per testare domini riservati, IP policy, OAuth) — maintainer.
- Account Apple Developer Program per firma e notarizzazione — maintainer.
- Crate `ngrok` (SDK ufficiale), `objc2` e famiglia, `tokio`, `hyper`, `reqwest` — ecosistema.
- Hosting del feed Sparkle (GitHub Releases/Pages).

**Blocker noti**
- Nessuno bloccante. Da chiudere con uno **spike tecnico in M0**:
  1. Conferma della copertura SDK per ogni riga della matrice di parità.
  2. Prototipo NSPopover + NSTableView con objc2 per validare l'approccio UI.
  3. Prototipo di reverse proxy dell'ispettore sopra un listener SDK.

---

## Appendix

### A. Matrice di parità CLI → Shaft

| Comando CLI ngrok v3 | Funzione in Shaft | v1.0 |
|---|---|---|
| `ngrok http/tcp/tls` (+ tutti i flag) | Quick tunnel + profili (Feature 2) | ✅ |
| `ngrok start <nomi>` / `--all` | Avvio profili / "Avvia tutti i preferiti" | ✅ |
| `ngrok config add-authtoken` / `add-api-key` | Gestione account (Portachiavi) | ✅ |
| `ngrok config check` / `edit` / `upgrade` | Validazione, editor, import v2→v3 | ✅ |
| `ngrok api <risorsa> <azione>` | Pannelli Risorse + Explorer API generico | ✅ |
| Ispettore web `:4040` / replay | Finestra Ispettore (Feature 3) | ✅ |
| `ngrok diagnose` | Finestra Diagnostica | ✅ |
| `ngrok version` | Info / Diagnostica | ✅ |
| `ngrok update` | Aggiornamento app via Sparkle | ✅ (equivalente) |
| `ngrok service` | — | ❌ (fuori scope) |
| `ngrok completion` | — | ❌ (non applicabile) |

### B. Glossario
- **Agente / Sessione**: connessione autenticata (authtoken) tra il client e il servizio ngrok; ospita uno o più endpoint/tunnel.
- **Endpoint / Tunnel**: indirizzo pubblico (o interno) che inoltra traffico verso un upstream locale.
- **Upstream**: servizio locale verso cui viene inoltrato il traffico (es. `localhost:3000`).
- **Traffic Policy**: regole dichiarative (YAML/JSON) applicate da ngrok al traffico di un endpoint (auth, header, rate limit, IP restriction…).
- **Cloud Endpoint**: endpoint definito via API che vive indipendentemente dagli agenti.
- **Authtoken**: credenziale per aprire sessioni agente. **API key**: credenziale per l'API REST.
- **Pooling**: più agenti che servono lo stesso endpoint con bilanciamento.
- **LSUIElement**: chiave Info.plist che rende l'app "agent" (niente Dock, niente menu app).

### C. Riferimenti
- Documentazione ngrok: https://ngrok.com/docs
- SDK Rust: https://github.com/ngrok/ngrok-rust — https://docs.rs/ngrok
- API REST ngrok: https://ngrok.com/docs/api
- Traffic Policy: https://ngrok.com/docs/traffic-policy
- objc2: https://github.com/madsmtm/objc2
- Sparkle: https://sparkle-project.org

---

*Questo PRD è stato redatto tramite raccolta interattiva dei requisiti con punteggio di qualità, per coprire in modo completo gli aspetti di business, funzionali, UX e tecnici.*
