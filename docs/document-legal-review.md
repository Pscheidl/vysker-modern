# Právní kontrola evidence dokumentů

Ověřeno 1. 11. 2026 pro změnu na větvi `pavel/unified-public-documents`.
Jde o kontrolu navrženého postupu a jeho technických pojistek. Nejde o právní
posouzení jednotlivých obecních písemností ani o ověření fyzické úřední desky.
Produkční obsah ani skutečná vyvěšení nebyly v této práci měněny.

## Jedna evidence a jednoduchý formulář

Jednotná databáze, editor a společný archiv jsou technické uspořádání. Důležité
je zachovat rozlišení skutečného vyvěšení, plánovaného sejmutí, skutečného sejmutí
a veřejné dostupnosti. Úředník nemusí tyto skutečnosti zadávat do dvou různých
částí administrace.

Metodika MV pro rok 2026 odlišuje fyzickou úřední desku od zpřístupnění jejího
obsahu na internetu. Pouhé zveřejnění na webu nedokládá úkon na fyzické desce.
Skutečné datum sejmutí se nemá zapisovat předem. Vyvěšený stejnopis a jeho
následné zařazení do spisu jsou samostatnou povinností. Web ani jeho interní
archiv nenahrazují spisovou službu.
[MV, Obecní úřad, metodika 13.1, strany 10 až 13](https://mv.gov.cz/mvcren/SCRIPT/ViewFile.aspx?docid=22550596)

Číslo jednací a datum vytvoření jsou v obecném katalogu nepovinné. To neznamená,
že mohou chybět náležitosti předepsané pro konkrétní vydávanou písemnost. Ty musí
obsahovat samotná písemnost. Odebrání povinných pomocných polí z editoru není
zrušením povinností obce. Neměnnost zveřejněných bajtů a audit jsou technické
pojistky původní podoby, samy nepotvrzují právní účinky vyvěšení.

## Počítání dní a opravená časová hranice

Při kontrole bylo nalezeno příliš časné automatické sejmutí: začátek dne
`datum vyvěšení + počet dní` nezachová celý poslední den. Nový výpočet znamená
**N celých kalendářních dní po dni vyvěšení**, sejmutí nastane nejdříve na začátku
následujícího dne v časovém pásmu Europe/Prague.

Příklad: vyvěšení 1. 11. 2026 a zadaných 15 celých dní znamená dostupnost po celý
16. listopad a plánované sejmutí 17. listopadu. Počet dní nemá výchozí hodnotu.
Importované skutečné události a výslovně zadaná historická data se neposouvají.

Tento výpočet **není univerzální kalkulačka zákonné lhůty**. U doručování veřejnou
vyhláškou je třeba zohlednit také posun posledního dne připadajícího na sobotu,
neděli nebo svátek. Rozšířený senát NSS tento závěr vztáhl v bodě 87 výslovně také
na § 25 odst. 2 správního řádu. Starší návody tvrdící automaticky opak nejsou
spolehlivým podkladem. Obsluha zadá odpovídající počet dní a zkontroluje vypočtené
datum. Aplikace nevyvozuje datum doručení, právní moci ani včasnost námitek.
[NSS, 4 Afs 264/2018-85, body 86 a 87](https://vyhledavac.nssoud.cz/DokumentOriginal/Html/700790)

Doba se může řídit jiným pravidlem než doručování veřejnou vyhláškou. Například
majetkové záměry podle § 39 zákona o obcích mají vlastní režim. Nelze všem
písemnostem předvolit 15 dní ani na všechny bez rozlišení aplikovat pravidla
správního doručování. Metodika MV pro obecní majetek rozebírá i započítávání
hraničních dnů. Technický automat nespoléhá na toleranci vadného zveřejnění.
[MV, Povinnosti obcí při nakládání s obecním majetkem, metodika 7.2, strana 52](https://mv.gov.cz/odk2/soubor/metodicke-doporuceni-k-cinnosti-usc-c-7-2-povinnosti-obci-pri-nakladani-s-obecnim-majetkem-podle-zakona-o-obcich.aspx)

## Archiv a osobní údaje

Volba administrátora, že přílohy zůstanou veřejné, sama nezakládá právní důvod
pro další zveřejňování osobních údajů. ÚOOÚ upozorňuje také na dostupnost kopií
po sejmutí prostřednictvím vyhledávačů. Proto ochrana proti indexování a ukládání
do cache pokrývá i nové společné adresy `/dokumenty`, jejich detaily a soubory.
Znalost původního odkazu nepřekoná aktuální kontrolu přístupu na serveru.
[ÚOOÚ, poznatek k doručování na úřední desce, 29. 10. 2020](https://uoou.gov.cz/poznatek-z-dozorove-cinnosti-k-dorucovani-pisemnosti-na-uredni-desce)

Volba **přílohy pouze pro administrátory** po sejmutí skryje soubory a jejich
názvy. Název a popis dokumentu mohou zůstat veřejné. Vysvětlivka na to výslovně
upozorňuje. Zjednodušený editor nenabízí ruční ukončení veřejnosti celého
záznamu ani vytvoření propojené kopie. Jestliže i název nebo popis vyžadují
znepřístupnění, musí oprávněný technický správce použít zachované chráněné API
pro změnu veřejné dostupnosti s uvedením důvodu. Samotná volba skrytých příloh
tuto potřebu neřeší. Existující propojené anonymizované znění zůstává samostatným
dokumentem a nepřepisuje původní bajty.

Oprávněný správce může originál dál stáhnout v administraci. Interní uchování
má vlastní účel a retenční režim. Nejde o povolení uchovávat osobní údaje navždy.
Zákaz indexování není technickou zárukou odstranění kopií, které už získal někdo
jiný. Veřejné důvody změn a auditní údaje se proto také musí posuzovat podle svého
obsahu.

Některé písemnosti musí zůstat na internetu dostupné déle než krátké vyvěšení.
Například schválený rozpočet má podle § 11 odst. 4 zákona č. 250/2000 Sb. vazbu
na zveřejnění rozpočtu následujícího roku a konec rozpočtového roku. Volbu skrytí
příloh proto nelze použít plošně. Společný veřejný katalog zachovává samostatné
řízení této dostupnosti i po archivaci.
[e-Sbírka, zákon č. 250/2000 Sb., § 11](https://e-sbirka.gov.cz/sb/2000/250)

## Co musí posoudit obec u konkrétní písemnosti

- Právní důvod a rozsah zveřejnění včetně osobních údajů v názvu, textu i přílohách.
- Potřebnou dobu, její konec a vazbu na projednání, doručení nebo jinou událost.
- Shodu a časovou návaznost zveřejnění na fyzické desce a na webu.
- Případnou povinnost pokračovat ve veřejném zpřístupnění po sejmutí.
- Uložení dokladů do spisu a příslušný retenční či skartační režim.

Tyto body vysvětlují hranice aplikace. Nevracejí do jednoduchého editoru právní
formulář ani nová povinná pole. Nejasný konkrétní případ je třeba vyřešit před
vyvěšením s odpovědným pracovníkem obce, případně s jejím právníkem či pověřencem.
