# Automatické přebírání odstávek ČEZ Distribuce

Import přidává plánované odstávky do běžného kalendáře, bez samostatné kategorie.
Používá existující kategorii `other`. Ukládá začátek a konec, místní části, čísla
popisná, evidenční a orientační, parcely, číslo oznámení a odkaz na původní PDF.
Veřejný detail i administrace uvádějí ČEZ Distribuce jako zdroj. Stejný obsah
se přenáší také do stávajícího odběru kalendáře a jednorázového exportu ICS.

## Zdroj, rozsah a cena

Ověřeno 29. 10. 2026. Čteme anonymní veřejné rozhraní, které používá oficiální
web ČEZ Distribuce, nikoliv registrované API pro obchodníky s elektřinou:

- [Přehled ČEZ pro celou obec Vyskeř](https://www.cezdistribuce.cz/cs/pro-zakazniky/potrebuji-vyresit/stavajici-pripojeni/overeni-planovane-odstavky?jlTown=577685)
- [Veřejná JSON odpověď](https://api.bezstavy.cz/cezd/api/inspecttown/577685)
- [Oficiální widget používající toto rozhraní](https://cdn.bezstavy.cz/assets/juiceless-widget.js)
- [Části obce v RÚIAN](https://vdp.cuzk.gov.cz/vdp/ruian/castiobce?kodOb=577685&search=)

Při ověření nebyla potřeba registrace, klíč ani platba. Nebyl nalezen veřejný
ceník, garantovaná dostupnost, pravidla četnosti dotazů ani závazek zachování
tohoto nezdokumentovaného rozhraní. Nejde tedy o smluvně garantované bezplatné
API. Import nepotřebuje další placenou službu, běží ve stávajícím webovém procesu.

Dotaz používá kód **celé obce 577685**, nikoli jednu adresu či odběrné místo.
Pokrývá Drahoňovice, Lažany, Mladostov, Poddoubí, Skalany a Vyskeř. Parser
prochází všechny místní části v odpovědi a nevybírá jen část nazvanou Vyskeř.
Konkrétní odstávka může zasáhnout pouze některé části nebo čísla domů.

Jedna odpověď může obsahovat i další obce. Do místního detailu vstupují pouze
adresy obce s přesně odpovídajícím kódem. Parcely se čtou jak ze seznamu katastrů
obce, tak z katastru uvedeného u místní části. Osiřelé katastry bez ověřené vazby
na obec se nepřiřazují. Originální PDF může popisovat celou širší odstávku.

## Zapnutí po review

Výchozí nastavení je vypnuté, včetně produkčního Compose. Pro schválený provoz:

```dotenv
OBEC_CEZ_ODSTAVKY_ENABLED=true
OBEC_CEZ_OBEC_KOD=577685
OBEC_CEZ_INTERVAL_SECONDS=3600
```

Po prvním spuštění server provede kontrolu. Při dalších restartech respektuje
uložený termín příští kontroly. Následně kontroluje nejvýše jednou za zadaný
interval. Výchozí interval je hodina, povolené rozmezí 900 až 86400 sekund.
Konfigurace se čte z prostředí procesu, `.env` se automaticky nenačítá.
Produkční Compose předává tyto tři hodnoty do služby `web`.

Stav je vidět ve správě kalendáře. Obsahuje poslední pokus, poslední úspěch,
termín další kontroly a případnou chybu. Tlačítko pro obnovení stavu pouze
načte stav, nevyvolává nový dotaz na ČEZ. Stavové API vyžaduje přihlášení:
`GET /api/v1/admin/cez-outages/status`.

Importované události jsou v administraci pouze ke čtení, aby další kontrola
nepřepsala ruční změny. Zastavení importu se provede nastavením `false` a
restartem. Již uložené události tím nezmizí.

## Změny, zrušení a chyby

- Identita je tvořena poskytovatelem, kódem obce a číslem odstávky. Opakované
  načtení nezakládá další událost. Stejná data nemění revizi ani ICS.
- Změna termínu, adres či oznámení aktualizuje stejnou událost se stejným UID.
  Časy z API se čtou s uvedeným offsetem a zobrazují v pásmu Europe/Prague.
- Výslovné `cancelled_at` označí událost jako zrušenou, ale zachová její veřejný
  detail. Tvar pole vychází z oficiálního widgetu. Test zrušení je syntetický,
  živá zrušená odstávka nebyla při ověření k dispozici.
- Budoucí nezrušená odstávka chybějící ve dvou úplných úspěšných odpovědích
  s odstupem alespoň jednoho intervalu se stáhne z veřejného kalendáře.
  Neoznačujeme ji tím za distributorem zrušenou. ICS odešle odstranění z obecního
  přehledu. Pokud se znovu objeví, obnoví se stejná událost a UID.
- Absence neskryje již probíhající, minulou nebo výslovně zrušenou událost.
- Síťová chyba, timeout, přesměrování, neplatná odpověď nebo neúplný detail
  ponechají kalendář beze změny. Celý snapshot se nejprve ověří. Zápisy,
  odebrání, stav a audit se poté uloží v jediné transakci.
- Databázový termín kontroly a token běhu brání souběžným dotazům více procesů
  i zápisu zastaralé odpovědi. Síťový požadavek nedrží databázový zámek.
- Požadavek má limit 20 sekund a 2 MiB. Následující pokus nastane až v běžném
  intervalu, bez agresivních opakování při chybě.

## Ověření

`tests/fixtures/cez-vysker.json` obsahuje veřejný vzorek odstávky č. 110061127971
z 29. 10. 2026, zahrnující všech šest místních částí a 23 parcel. Prázdný vzorek
Ktové má podobu `{"outages":null}`. Parser ji odlišuje od chybějících dat
v neúplné odpovědi.

Jednotkové testy ověřují adresy, parcely, změny pořadí, časová pásma, zrušení,
bezpečné odkazy, neúplné odpovědi a hraniční časy. `tests/power_outages.rs`
ověřuje synchronizaci v izolované databázi, opakování, změny, ICS, stažení,
návrat, chyby, rollback a souběh procesů. Přímý síťový test je mimo běžnou sadu:

```sh
cargo test --locked --no-default-features --features ssr --lib \
  backend::cez::tests::live_public_api_snapshot_parses -- --ignored --nocapture
```

Testovací databáze se nesmí nahrazovat produkční ani běžnou vývojovou databází.
