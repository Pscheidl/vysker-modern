# Návrh přístupu pro LLM a MCP

Schválený návrh z 3. listopadu 2026. Ve zdrojovém kódu je implementována
první veřejná verze určená pouze ke čtení. Tento dokument popisuje její
rozsah a odůvodnění, nepotvrzuje dokončené testování ani konkrétní nasazení.
Aktuální návod pro klienty je v [anglickém průvodci](for-agents.md),
provozní omezení v [provozním návodu](operations.md#public-mcp-and-agent-discovery).

Cílem je umožnit agentovi vyhledat veřejné informace, přečíst jejich zdroje,
správně je citovat a zjistit dostupnost pronájmu. První verze zpřístupňuje čtení.
MCP používá existující databázi a aplikační pravidla. Na VPS kvůli němu
nemusí běžet jazykový model ani další databáze.

## Podklady a implementované doplnění

| Podklad | Existující rozhraní | Doplnění v první verzi |
| --- | --- | --- |
| Vyhledávání | `GET /api/v1/search`, názvy mají přednost před obsahem příloh | Anglický průvodce a OpenAPI s parametry a významem počtu výsledků |
| Dokumenty | `GET /api/v1/documents` a `/{id}`, metadata, přílohy, související dokumenty | MCP čtení se stránkováním a zdrojovými odkazy |
| Texty příloh | `GET /api/v1/attachments/{id}/text`, kompletní veřejný přepis | Úplný přepis v mezích velikosti odpovědi, jinak pokračování kurzorem, stránky a metoda extrakce |
| Stránky obce | `GET /api/v1/pages/{slug}` s Markdown v poli `content` a serverem vykreslené HTML | `/api/v1/pages/{slug}/markdown` a odkaz na alternativní reprezentaci |
| Kalendář | Veřejné rozhraní událostí | MCP čtení s omezeným rozsahem a stránkováním |
| Pronájmy | Majetek a dostupnost přes `/api/v1/rental-assets` | MCP čtení intervalů a odkaz do rezervačního formuláře |

Vyhledávání dnes přijímá nejvýše 200 znaků a 10 slov, vrací 20 výsledků
na stránku a používá parametry `q`, `kind`, `state`, `category`, `sort`, `page`.
Nemá sémantické hledání ani filtr rozsahu dat. `total` je počet dokumentů
a stránek. Není to počet hlasování ani všech výskytů hledaného slova.
Seznam dokumentů už má `limit` a `offset`, výchozí limit je 50, maximum 100.

Před implementací při kontrole veřejného webu 3. listopadu vracely `/llms.txt`,
`/.well-known/ai-catalog.json`, `/mcp/server-card`, `/robots.txt` a `/sitemap.xml`
stav 404. Domovská stránka měla hlavičku `X-Robots-Tag: noindex, nofollow`.
Implementace nastavení veřejné indexace nemění. Dříve auditovaná větev
neobsahovala MCP, OpenAPI ani JSON-LD. Nové MCP a OpenAPI jsou součástí
této implementace. Exporty obecních otevřených dat z jiných lokálních větví
tento čtecí server nezpřístupňuje.

## Jak agent zjistí, co web nabízí

Stránka `/for-agents`, její Markdown varianta `/for-agents.md` a `/llms.txt`
popisují účel webu, provozní režim, jazyky, rozdíl mezi Dokumenty, Úřední
deskou a Archivem, hledání a texty příloh. Upozorňují, že neznámé datum
historického vyvěšení nelze nahrazovat datem importu. Patička odkazuje na
průvodce a HTML hlavička na `/llms.txt` pomocí `rel="describedby"`.

`/for-agents` je naše volba adresy rozcestníku, nikoli standardizovaná cesta.
Technický návod, popisy nástrojů a průvodní text `/llms.txt` jsou anglicky.
Angličtina je návrhové rozhodnutí, nikoli požadavek MCP nebo konvence llms.txt.
Dokumenty obce a citované pasáže zůstávají v původní češtině, s uvedením jazyka.

`llms.txt` je veřejný návrh konvence. Pomůže klientům, které jej používají,
ale nezaručuje, že ho každý model sám načte. Jeho autor doporučuje i čisté
Markdown varianty a odkazy `rel="alternate"` s typem `text/markdown`.
[Návrh llms.txt](https://llmstxt.org/).

`/openapi.json` popisuje existující veřejné čtecí HTTP cesty, jejich
parametry, stránkování, datové typy a chyby. MCP není součástí této REST
specifikace. Identita a podporované verze MCP v kartě vycházejí ze stejných
konstant jako běžící server. HTML a Markdown průvodce používají společný
textový zdroj.

Metadata se generují z nastavené veřejné URL serveru. Implementované cesty:

| Adresa | Účel |
| --- | --- |
| `/mcp` | Vlastní MCP endpoint |
| `/.well-known/ai-catalog.json` | Doménový katalog s odkazem na kartu MCP |
| `/mcp/server-card` | Identita serveru, endpoint, transport a podporované verze |

Implementace používá pro katalog `application/ai-catalog+json` a pro kartu
`application/mcp-server-card+json`. Karta neobsahuje úplný seznam nástrojů,
ten klient získá při komunikaci se serverem. Katalog i karta vznikají
z konfigurace skutečného serveru a sdílených protokolových konstant.
[Discovery návrh](https://github.com/modelcontextprotocol/ext-server-card/blob/main/docs/discovery.md).

Server Cards jsou stále označené jako experimentální, nepřijaté rozšíření.
Nelze slíbit automatické připojení po návštěvě libovolné stránky. Klient musí
podporovat nalezení, připojení a příslušný způsob autorizace. Přímá adresa
MCP a návod připojení zůstávají nutným záložním způsobem.
[Stav Server Cards](https://github.com/modelcontextprotocol/ext-server-card).

`server/discover` v aktuálním MCP zjišťuje schopnosti a verze serveru, jehož
adresu už klient zná. Nejde o hledání endpointu na doméně.
[Specifikace server/discover](https://modelcontextprotocol.io/specification/2026-07-28/server/discover).

## Implementované MCP

MCP je modul stávající aplikace Axum se sdíleným databázovým poolem.
Používá oficiální Rust SDK `rmcp`, zafixované na `3.8.0`, s podporou
protokolu `2026-07-28` a starších revizí uvedených v generované kartě.
[Rust SDK](https://github.com/modelcontextprotocol/rust-sdk).

Transport je Streamable HTTP na `/mcp`, s krátkými JSON odpověďmi bez
potřeby dlouhodobého stavu spojení. Současná revize používá POST požadavky
a nepotřebuje původní handshake ani protokolové sessions. SDK zajišťuje
kompatibilitu starších klientů. Transport kontroluje `Origin` a omezuje
požadavky. CORS veřejných metadat je oddělený od administrace.
[Streamable HTTP](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http).

První verze implementuje devět nástrojů. Přesné vstupní schéma poskytuje
`tools/list`:

| Nástroj | Vstup | Výsledek |
| --- | --- | --- |
| `search` | Dotaz, kolekce, kategorie, řazení, stránka | Shody, úryvky, zdrojové odkazy, další stránka a počet nalezených dokumentů |
| `list_documents` | Kolekce, stránkování | Veřejné dokumenty a přílohy včetně dostupnosti přepisu |
| `get_document` | ID dokumentu, úsek popisu a očekávaná verze | Metadata, zveřejnění, přílohy a související dokumenty |
| `get_page` | Slug stránky, úsek obsahu a očekávaná verze | Čitelný obsah a kanonická URL |
| `read_attachment_text` | ID přílohy, volitelná stránka, kurzor nebo omezení délky | Úplný přepis, pokud se vejde, jinak části s pokračováním, označení stránek a informace o OCR |
| `get_content_coverage` | Kolekce | Stav převodu pouze aktuálně veřejných příloh, nepodporované formáty a čas zjištění |
| `list_events` | Omezený časový rozsah, stránkování | Veřejné události a jejich odkazy |
| `list_rental_assets` | Bez povinných parametrů | Aktivní pronajímaný majetek, pravidla a ceny |
| `get_rental_availability` | ID majetku, časový rozsah | Obsazené intervaly bez osobních údajů a odkaz do formuláře |

Kolekce zachovávají rozlišení Dokumenty, Úřední deska, Archiv úřední desky
a stránky. Hledání přebírá současné pořadí shod. Nové filtry, například
rozsah dat, musí nejdříve získat skutečnou podporu ve společné čtecí vrstvě.
Datum zveřejnění není automaticky vydáváno za datum jednání nebo hlasování.

Pro první verzi stačí tyto nástroje. Volitelné MCP resources mohou později
nabídnout tutéž čtecí logiku přes šablony URI dokumentů a textových částí.
Nesmí zavést další nezávislá pravidla viditelnosti.

## Čitelná odpověď a původ informací

Výsledky obsahují stabilní ID, kanonickou URL, název zdroje, čas načtení
a případná známá data dokumentu. Text přílohy navíc uvádí ID a název přílohy,
číslo stránky nebo označení části, metodu extrakce, kontrolní součet zdroje
a informaci, zda jde o úplný obsah nebo výřez. U DOCX/ODT nenazývat logickou
část fyzickou stránkou.

Přepis přílohy se ve výchozím stavu vrací celý, pokud se jeho data vejdou
do 96 KiB. Delší přepis pokračuje přes `next_cursor`, dokud je `has_more`
pravdivé. Pole `complete` říká, zda odpověď zahrnula všechny zdrojové části
od začátku. Volitelné `max_chars` omezuje odpověď na 1 až 32 000 znaků.
`text_url` vždy vede na kompletní dostupný veřejný přepis. Kurzor je svázaný
s přílohou a verzí přepisu, při změně klient načte nový začátek.

Popis dokumentu a obsah stránky používají výchozí úsek 12 000 znaků
a maximum 32 000 znaků, s pokračováním podle znakového offsetu a verze.
Události i dostupnost používají časový rozsah nejvýše 366 dní a stránkování
po 1 až 100 položkách, standardně po 50. Události podporují i minulost,
obsazenost pronájmu začíná nejdříve dneškem stejně jako veřejný kalendář.

Chybějící text není prázdný dokument. Rozlišit připravovaný, neúspěšný
a nepodporovaný převod u veřejné přílohy. Neveřejná či chybějící příloha
zůstane nerozlišitelná stejnou chybou nedostupnosti. Existující administrační
přehled pokrytí nelze přímo vystavit, protože zahrnuje i koncepty.

Pro otázku „Kolikrát se hlasovalo o pronájmu hospody?“ má agent:

1. Vymezit období, projít soupis dostupných zápisů a zkontrolovat pokrytí.
2. Vyhledat více formulací tématu, protože současné hledání nerozumí synonymům.
3. Přečíst relevantní části úplných zápisů, nejen úryvek z výsledku hledání.
4. Rozlišit hlasování od zmínky, dalšího projednání a opakované kopie dokumentu.
5. U každého započítaného hlasování uvést datum, bod či usnesení, zdroj a stránku.
6. U neúplného pokrytí odpovědět například „V dostupných podkladech jsem našel…“.

Počet výsledků hledání nesmí sloužit jako počet hlasování. Ani 100% technické
pokrytí přepisy samo nezaručuje úplnost významového vyhodnocení. Ověřený
registr jednotlivých usnesení a hlasování by byl samostatnou navazující funkcí.

## Přístup, rezervace a provoz

Veřejné čtecí MCP vrací stejná data jako anonymní návštěvník webu.
Při každém čtení znovu používá `document_is_public` a `document_files_public`.
Přihlášení správce přes běžnou cookie jeho veřejný rozsah nezvětšuje. Texty
z dokumentů zůstávají daty, nesmí se proměnit v instrukce serveru pro agenta.

Čtecí nástroje nemění dokumenty, neodesílají e-maily, neresetují neúspěšné
převody ani nespouštějí OCR během požadavku. Server omezuje souběžná čtení
na čtyři, čas čtení na deset sekund, tělo požadavku na 128 KiB a výslednou
MCP odpověď na 256 KiB. Limit je 60 požadavků za minutu podle klientské IP.
Tyto meze nejsou výsledkem měření na VPS, provozní odezvu je třeba ověřit.

Pro rezervace první verze zjistí obsazenost a odkáže uživatele do existujícího
formuláře. Současné vytvoření žádosti vyžaduje osobní údaje, doklad, souhlas
a vyvolává e-maily. Žádost čeká na schválení, není hotovou rezervací. MCP musí
respektovat vypnutí rezervačního systému a nikdy nevracet údaje jiných nájemců.

Případné budoucí vytvoření žádosti přes MCP potřebuje vlastní oprávnění,
náhled konkrétní žádosti, výslovné potvrzení a ochranu před duplicitním
odesláním. Doklad je vhodnější nahrát přímo do zabezpečeného formuláře webu.
Samotná návštěva stránky nebo nalezení karty MCP není souhlas s rezervací.

WebMCP lze později posoudit jako doplněk pro agenta pracujícího v otevřené
kartě. Jde o samostatný experimentální návrh browserového rozhraní, nikoli
podmínku tohoto serveru nebo standard podporovaný všemi prohlížeči.
[WebMCP, draft z 2. listopadu 2026](https://webmachinelearning.github.io/webmcp/).

## Ověření a další kroky

Součástí změny jsou integrační testy transportu, veřejné čtecí vrstvy
a metadat a test anglického průvodce bez JavaScriptu na mobilním rozměru.
Jejich přítomnost není tvrzením, že již všechny prošly. Výsledek konkrétního
běhu a nasazení musí být doložen samostatně. Veřejná indexace, registr MCP
a případné zápisové nástroje zůstávají samostatným rozhodnutím.

Před nasazením ověřit moderní i starší klienty, pořadí shod, čtení všech
podporovaných typů příloh, pokračování dlouhého textu a změnu zdroje mezi
požadavky. Kontrolovat skrytí dokumentu a souboru také po předchozím úspěšném
čtení, neúplné OCR, nedostupný rezervační systém a zachování prázdné fronty
nových e-mailů při čtecích voláních. Změřit odezvu a paměť při souběhu
MCP dotazů, běžného webu a stávajícího OCR workeru.

Vlastní MCP nevyžaduje další OCR index ani stahování a opakované převádění
stejných souborů. Největší část potřebných dat již existuje.
