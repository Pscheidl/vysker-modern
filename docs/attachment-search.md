# Vyhledávání v přílohách

Současné vyhledávací pole hledá v názvech a popisech dokumentů, publikovaných
stránkách i v získaném textu veřejných příloh. Originální soubory se nemění.
Odvozený text je uložen v existující PostgreSQL, která jej prohledává pomocí
trigramového indexu. Není potřeba jazykový model ani další databázový server.

## Podporované formáty

| Formát | Zpracování | Označení místa ve výsledku |
| --- | --- | --- |
| PDF s textem | Poppler získá existující text | Fyzické číslo stránky |
| Naskenované PDF | Stránky se postupně vykreslí a přečtou pomocí českého a anglického OCR | Fyzické číslo stránky |
| Smíšené PDF | Rozhodování po stránkách, včetně stránky s nativním textem a velkým obrázkem | Fyzické číslo stránky |
| DOCX | Text, tabulky, poznámky a záhlaví | Dokument nebo poznámky a záhlaví |
| ODT | Text a tabulky | Dokument |
| XLSX, ODS | Textové a uložené číselné hodnoty buněk | Název listu |
| CSV | Řádky a sloupce, včetně hodnot v uvozovkách | Tabulka CSV |
| TXT | Text UTF-8 | Text |
| JSON, XML | Názvy položek a jejich hodnoty | Data JSON nebo Data XML |

U DOCX a ODT se neuvádí fyzické číslo stránky, které bez úplného vykreslení
kancelářským programem nelze spolehlivě určit. U tabulek slouží první neprázdný
řádek jako popisky dalších hodnot. Vzorce ani makra se nespouštějí. XLSX používá
uložené výsledky vzorců, pokud jsou přítomné, a nepřepočítává je.

TXT a CSV musí být v UTF-8. XML s DTD nebo deklaracemi entit se odmítá. JSON
s duplicitními názvy položek se odmítá, aby se část obsahu neztratila bez upozornění.
Starší binární DOC a XLS, RTF ani obrázkové přílohy samostatně indexovány nejsou.
OCR může chybovat, zvláště u špatných skenů, rukopisu a drobného písma.

## Pořadí a podoba výsledků

Při řazení podle relevance má přednost přesný název, potom shoda všech hledaných
slov v názvu, částečná shoda názvu, název přílohy, metadata a nakonec obsah přílohy.
Název souboru lze vyhledat i před dokončením extrakce jeho obsahu.
Mnoho výskytů uvnitř přílohy nepřeváží shodu v názvu. Hledání zachovává
nerozlišování velikosti písmen a české diakritiky, používá doslovné části slov.
Automatická synonyma ani české skloňování nejsou součástí této změny.

Jeden dokument je ve výsledcích jednou, i když odpovídá více stran nebo příloh.
Shoda v příloze ukáže její název, místo a krátký úryvek. U PDF lze odkazem přejít
na odpovídající stránku, pokud to prohlížeč PDF podporuje. Ručně zvolené řazení
podle data nebo názvu zůstává dostupné.

## Veřejný textový přepis přílohy

Dokončený přepis patří ke stejné příloze jako původní soubor. Lze jej přímo
otevřít jako text UTF-8 v nové kartě nebo stáhnout jako soubor TXT, bez JavaScriptu:

- Náhled: `GET /api/v1/attachments/{id}/text`
- Stažení: `GET /api/v1/attachments/{id}/text/download`

Přepis obsahuje původní název souboru, odkaz na nadřazený dokument a originál,
všechny stránky nebo části v původním pořadí a upozornění, pokud bylo použito
OCR. Text XML nebo HTML uvnitř přílohy se zobrazuje doslova. Přepis se nesnaží
zachovat grafickou podobu originálu.

Oba odkazy před každým zpřístupněním znovu kontrolují aktuální oprávnění
původní přílohy a dokumentu. Administrátorské přihlášení tuto veřejnou kontrolu
neobchází. Soukromý, odstraněný, nedokončený, neúspěšný nebo zastaralý přepis
vrací 404. Odpověď zakazuje ukládání do cache. Výměna nebo přejmenování přílohy
zneplatní její dosavadní přepis ve stejné databázové transakci.

## Průběh v administraci

Na konci úvodního přehledu administrace a seznamu dokumentů je panel
„Vyhledávání v přílohách“. Ukazuje připravené přílohy z celkového počtu,
čekající a právě zpracovávané soubory i neúspěšné pokusy. Procento vyjadřuje
počet skutečně dokončených příloh. Neúspěch se nezapočítává jako hotový přepis.

Stav se automaticky načítá každé tři sekundy, po odchodu ze stránky se obnova
zastaví. Při krátkém výpadku připojení zůstává viditelný poslední známý stav.
Průběh přibývá po celých souborech, nejde o odhad postupu uvnitř jedné stránky OCR.
Odkazy u zpracovávaných a neúspěšných příloh vedou k jejich dokumentu.

Přehled zahrnuje podporované soubory čekající na přípravu, včetně konceptů.
Text konceptů se tím nezveřejňuje. Přístup k přehledu vyžaduje přihlášení správce.

Získání textu, opakování neúspěšného OCR ani nové vytvoření indexu nemění
publikovaný dokument, jeho kontrolní součet, historii ani e-mailové oznámení.
Regresní test `attachment_mail_silence` ověřuje také nezměněnou frontu zpráv
a to, že následující příprava pravidelného přehledu nevytvoří další e-mail.

## Samostatná služba a limity

Program `attachment-index` zpracovává soubory mimo webové požadavky, po jednom.
Databázový zámek brání souběhu dvou indexovacích procesů nad stejnou databází
a schématem. Nové soubory objeví i po importu původního webu, včetně dříve
nahraných příloh bez indexu. Nečinná služba kontroluje práci po 15 sekundách.

| Omezení | Výchozí hodnota |
| --- | --- |
| Paměť celé služby v Compose | 512 MiB včetně podprocesů a dočasných souborů v paměti |
| Swap služby v Compose | Vypnutý |
| CPU služby v Compose | Nejvýše ekvivalent jednoho jádra |
| Vlákna OCR | Jedno |
| Adresní prostor jednoho extrakčního procesu | 384 MiB |
| Celková doba zpracování jednoho souboru | 3 600 sekund, tedy jedna hodina |
| Vstupní soubor | 10 MiB |
| Stránky PDF nebo listy/části dokumentu | Nejvýše 200 |
| Získaný text | Nejvýše 2 000 000 znaků |
| Rozbalený kancelářský dokument | Nejvýše 64 MiB a 5 000 položek |
| Tabulka | Nejvýše 100 000 řádků a 1 024 sloupců |
| Dočasné úložiště služby v Compose | 96 MiB |

Limit adresního prostoru jednoho procesu není limit součtu pamětí. Souhrnný
limit 512 MiB vynucuje kontejner nebo samostatná skupina systemd. Samotné
spuštění Python skriptu bez této skupiny souhrnný limit nenastavuje.

Pro každý soubor se spouští nové extrakční procesy. Tesseract a Poppler se po
své práci ukončí, po dokončení souboru skončí i Python a odstraní se jeho
pracovní soubory. OCR tedy při čekání na další dokument nedrží načtená jazyková
data ani vykreslené stránky. Zůstává malá služba sledující frontu, takže její
celková spotřeba neklesá na nulu. Kontroluje se aktuální paměť a seznam procesů,
nikoli historické maximum `memory.peak`, které po dokončení neklesá.

Při překročení limitu, nečitelném souboru nebo chybě zpracování se výsledek označí
jako neúspěšný. Částečný text se nevydává za úplný index. Překročení doby ukončí
také podprocesy OCR a odstraní dočasné soubory. Neúspěch jedné přílohy neblokuje
další soubory ani zveřejnění a stahování původního dokumentu.

Služba vyžaduje Python 3, `poppler-utils`, `tesseract-ocr`, `tesseract-ocr-ces`
a `tesseract-ocr-eng`. Vývojové prostředí `nix develop` obsahuje stejné nástroje i jazyky OCR.
Provozní Docker obrazy obsahují potřebné programy i
extrakční skript. Compose konfigurace pro staging a produkci obsahují službu
`attachment-index` se společnými přístupovými údaji k databázi, ale oddělenými
limity prostředků. Nasazení těchto konfigurací je samostatný provozní krok.

## Ruční spuštění a kontrola

Databázové připojení předejte proměnnou `OBEC_DATABAZE` nebo cestou k tajnému
souboru v `OBEC_DATABAZE_FILE`. Nenastavujte obě současně a nevkládejte heslo
do historie příkazového řádku. Program při startu provede migrace.

```bash
cargo build --features ssr --bin attachment-index
target/debug/attachment-index --once
```

`--once` zpracuje všechny právě způsobilé soubory a skončí. `--loop` běží trvale.
Volby `--memory-mb`, `--timeout-seconds`, `--max-pages` a `--extractor` umožňují
změnit limity nebo cestu ke skriptu. Alternativně cestu nastaví
`OBEC_ATTACHMENT_EXTRACTOR`.

Zkouška jediného souboru bez databáze:

```bash
python3 scripts/extract_attachment.py \
  --input tests/fixtures/search/scan.pdf --name scan.pdf \
  --max-memory-mb 384 --timeout-seconds 3600
```

Příkaz vypíše JSON se stavem `ready` nebo `failed`. Úspěšný výsledek obsahuje
stránky/části, způsob získání textu a případné upozornění na OCR. Výstup není
určen pro veřejné logy, protože obsahuje celý text dokumentu.

Přehled indexace v databázi:

```sql
SELECT status, count(*) FROM attachment_extractions GROUP BY status;

SELECT attachment_id, source_name, attempts, error, next_attempt_at
FROM attachment_extractions
WHERE status = 'failed'
ORDER BY attachment_id;
```

Stavy jsou `processing`, `ready` a `failed`. První neúspěch se opakuje nejdříve
za pět minut, druhý nejdříve za třicet minut. Po třech pokusech se automatické
opakování zastaví. Přerušené zpracování služba při dalším spuštění obnoví,
při zachování omezeného počtu pokusů.

Po opravě nástrojů nebo zvýšení vhodného limitu lze povolit nový pokus:

```sql
UPDATE attachment_extractions
SET attempts = 0, next_attempt_at = now()
WHERE status = 'failed';
```

Pro úplné nové zpracování jedné přílohy smažte jen její odvozený záznam.
Číslo `42` nahraďte ID požadované přílohy:

```sql
DELETE FROM attachment_extractions WHERE attachment_id = 42;
```

Navázané řádky `attachment_search_pages` se odstraní automaticky. Původní
soubor zůstane zachovaný a služba jej znovu zařadí. Změna obsahu nebo názvu
přílohy zneplatní starý index automaticky. Změna verze extraktoru také vyvolá
nové zpracování existujících záznamů.

## Veřejnost a uchování dat

Vyhledávání před každým vrácením shody kontroluje stejná pravidla veřejného
přístupu jako stažení přílohy. Připravený index sám nezpřístupní koncept,
neveřejnou přílohu ani dokument po skončení povolené doby zveřejnění.
Tyto podmínky platí i pro počty výsledků a úryvky.

Text je odvozený obsah přílohy. Při odstranění původních dat, označení přílohy
jako neveřejné nebo její výměně jej databázový trigger odstraní v téže transakci.
U časového omezení zpřístupnění se veřejné hledání zablokuje ihned podle data,
nezávisle na okamžiku dalšího běhu úklidu.

## Opakovatelné měření

Skript `scripts/benchmark_attachment.py` zpracuje právě jeden soubor a vypíše
JSON souhrn bez jeho textového obsahu. Uvádí dobu, počet stran, množství textu,
maximální paměť jednotlivého procesu a dostupné souhrnné metriky cgroup v2.

```bash
python3 scripts/benchmark_attachment.py tests/fixtures/search/scan.pdf \
  --contains hospody --output /tmp/attachment-benchmark.json
```

Pro porovnatelné souhrnné měření spusťte každý soubor v nové samostatné skupině:

```bash
systemd-run --user --wait --pipe --collect \
  -p MemoryMax=512M -p MemorySwapMax=0 -p CPUQuota=100% \
  --working-directory="$PWD" \
  python3 scripts/benchmark_attachment.py tests/fixtures/search/scan.pdf \
  --contains hospody --output /tmp/attachment-benchmark.json
```

Skupina musí mít k dispozici potřebné nástroje v `PATH`. Dostupnost uživatelského
systemd a paměťového řadiče závisí na prostředí. Pokud cgroup v2 není přístupná,
souhrnné hodnoty zůstanou prázdné, zatímco čas a paměť jednotlivých procesů
se změří dál.

`memory.peak` je maximum celé skupiny od jejího vzniku. V dlouho běžící sdílené
skupině může zahrnovat starší nebo nesouvisející práci, takže není měřením pouze
posledního souboru. `aggregate_current_sampled_peak_bytes` je maximum
`memory.current` vzorkované po 50 ms, které může přehlédnout kratší špičku.
`max_individual_process_rss_kib` je maximum jednotlivého procesu, nikoli součet
pamětí současně běžících procesů. `event_delta` ukazuje nové zásahy limitů
a případná ukončení kvůli nedostatku paměti.

### Výsledky místního měření z 1. listopadu 2026

Měřeno na Intel Core i7-10870H při omezení celé skupiny na 512 MiB, bez swapu
a s CPU limitem odpovídajícím jednomu jádru. Test obsahoval hustě popsané české
strany A4, včetně textových a obrazových verzí stejných podkladů.

| Podklad | Celková doba | Souhrnná špička paměti skupiny |
| --- | ---: | ---: |
| 10 stran textového PDF | 0,274 s | 33,7 MiB |
| 1 strana skenovaného PDF | 2,925 s | 116,0 MiB |
| 10 stran skenovaného PDF | 27,123 s | 115,9 MiB |
| Skutečný čtyřstránkový zápis Vyskeř 2/2022 | 9,532 s | 133,5 MiB |

Skutečný [zápis Vyskeř 2/2022](https://vysker.cz/assets/File.ashx?id_org=18774&id_dokumenty=1844)
má 1,52 MB, čtyři skenované A4 a žádnou textovou vrstvu. OCR získalo 8 168 znaků.
Běžné české věty a datum byly ověřeny proti obrazu. Přeškrtnutý text obsahoval
chyby rozpoznání. Stažený originál a výstup zůstávají pouze v `target/search-benchmark/`,
nejde o součást verzovaných testovacích podkladů. Zpráva je
`real-minutes-2022-02-report.json`.

Kontrolní výraz byl nalezen na všech očekávaných stranách. V těchto čtyřech bězích
nedošlo k dosažení paměťového limitu ani k ukončení procesu kvůli paměti.
Skupinové hodnoty zahrnují měřicí program, Python a extrakční nástroje,
nezahrnují web a PostgreSQL. Konkrétní naměřené výsledky pracovního běhu jsou
v `target/search-benchmark/*-result.json`.

Na hotové místní instanci prošlo také všech 13 příloh celým samostatným
indexovacím procesem. Vynucený limit byl 512 MiB, bez swapu, jedno CPU.
Špička celé služby včetně programu v Rustu byla 151,0 MiB, po dokončení
fronty spotřeba klesla přibližně na 21,1 MiB. Web a PostgreSQL běží odděleně.

Další kontrola běžící testovací služby při opětovném zpracování `scan.pdf`
naměřila celkovou paměť skupiny 11,9 MiB před úlohou, špičku 135,3 MiB a
35,4 MiB po dokončení a pěti sekundách klidu. Anonymní paměť procesů přitom
klesla ze špičky 100,9 MiB na 1,6 MiB, téměř k výchozím 1,4 MiB. Ve skupině
zůstal pouze indexer a jeho vývojový obal `bwrap`, žádný Python, Tesseract,
Poppler ani `timeout`. Celkový údaj zahrnuje také souborovou cache, sdílené
knihovny a paměť jádra, proto jeho zbytek není důkazem ponechaného OCR modelu.
Měření je uloženo v `target/search-preview/idle-memory.json`.

VPS bylo samostatně ověřeno jako 2 vCPU AMD EPYC Milan, 1 905 MiB RAM,
v okamžiku kontroly 1 080 MiB dostupných. OCR na VPS zatím měřeno nebylo.
Pro plánování dávky počítáme konzervativně přibližně **5 až 10 sekund na běžnou
skenovanou stránku**, tedy **50 až 100 sekund na deset stran**. Jde o odhad
s rezervou pro sdílené vCPU a provoz webu, nikoli o změřený poměr výkonu strojů.
Složitější, poškozené nebo špatně čitelné dokumenty mohou trvat déle a narazit
na limit jedné hodiny. Vyšší počet stran nezvyšuje paměť přibližně úměrně,
protože vykreslování a OCR probíhá po jedné stránce.

## Testy

```bash
python3 -m unittest tests/test_attachment_extractor.py -v
cargo test --features ssr --test attachment_index --test attachment_formats --test search
pnpm exec playwright test tests/browser/attachment-search.spec.ts
```

Pro plnou sadu musí být dostupné OCR nástroje a česká i anglická jazyková data.
Python testy OCR přeskočí, pokud chybí Tesseract nebo Poppler, proto při
ověřování nasazení kontrolujte také počet přeskočených testů. Databázové
a prohlížečové testy používejte s místním testovacím prostředím popsaným
v `docs/testing.md`.

### Závěrečné ověření implementace

Dne 1. listopadu 2026 prošlo 424 testů Rust/API, 11 testů extrakce,
5 testů balení a 3 prohlížečové scénáře. Sedm testů vyžadujících zvláštní
SMTP nebo zátěžové podmínky zůstalo ve výchozím ignorovaném režimu.
Bylo ověřeno všech devět formátů, čtyři varianty PDF, mobilní rozložení,
vyhledávání bez JavaScriptu i zachování změny formuláře během jeho načítání.
Compose konfigurace vývoje, stagingu a produkce prošly kontrolou syntaxe.
Docker obrazy se v tomto místním ověření nesestavovaly a VPS se neměnilo.
