# Pracovní 3D reliéf Vyskeře

Lokální stránka `/relief` navazuje na běžný veřejný web. Dosud nebyla přidána do navigace. Je pracovním náhledem, nikoli přesnou mapou. Zástavba a komunikace se mohou od aktuální skutečnosti lišit.

## Podklady a změny

- Výškopis DMR 5G, ČÚZK. Převzatá pravidelná síť 140 × 100 bodů ze samostatného prototypu. Rozsah 15.12 až 15.19 °E a 50.51 až 50.55 °N, výšky jsou pro čitelnost přibližně dvojnásobně zvýrazněny. Původní postup odběru a datum výškopisného zdroje zatím nejsou doloženy. Před publikací je nutné ověřit původní požadavek a metadata konkrétní datové sady.
- [RÚIAN, obec 577685](https://services.cuzk.gov.cz/shp/obec/epsg-5514/577685.zip), archiv stažen 30. října 2026, data z 19. října 2026. Vrstva `OBEC_P` poskytuje hranici a `CO_B` definiční body všech šesti částí obce. Hranice se celá nachází v použitém výřezu terénu. Tyto body se u několika osad mírně liší od orientačních souřadnic původního prototypu.
- [Katastrální mapa, k. ú. 787744](https://services.cuzk.gov.cz/shp/ku/epsg-5514/787744.zip), archiv stažen 30. října 2026, data z 25. října 2026. Vrstva `BUDOVY_P` obsahuje 406 půdorysů. Model používá jejich geometrii. Výšky se odhadují z údajů o počtu podlaží při dostupné vazbě na OSM, jinak z výchozí hodnoty. Většina střech je geometricky vytvořena nad skutečným půdorysem jako sedlová. Její výška vychází ze šířky půdorysu a štíty navazují na stěny. Počet řad oken se přizpůsobuje zaznamenaným podlažím. Při známém typu stavby jsou hospodářské budovy a některé chaty barevně odlišené, což je výtvarná volba. Umístění oken a dveří, barvy i tvary střech jsou stylizované a nemusí odpovídat skutečnosti. U staveb v OSM označených jako ploché zůstává střecha plochá. Kaple je samostatný stylizovaný model, její poloha a orientace vyžadují další ověření.
- [OpenStreetMap](https://www.openstreetmap.org/copyright), výřez stažen 30. října 2026 z `https://api.openstreetmap.org/api/0.6/map?bbox=15.12,50.51,15.19,50.55`. Komunikace a vodní toky jsou odvozené z linií `highway` a `waterway`, zjednodušené, oříznuté na rozsah terénu a promítnuté na výškopis. Vodní plochy vycházejí z dostupných uzavřených polygonů. Věžický rybník (OSM `way/50944666`) zasahuje za východní okraj výřezu, proto model zobrazuje pouze jeho západní část a značka leží uvnitř zobrazené části. Lesy vycházejí z uzavřených polygonů `landuse=forest` a `natural=wood`, vzorkovaných na síť reliéfu. Multipolygony a jiné typy ploch nejsou úplně zastoupené. Stromy se nepřidávají a pro zachování schválené podoby zůstává vrchol Hůry bez lesního odstínu. Šířky a barvy jsou ilustrativní. Místní názvy, vybrané body a dostupný počet podlaží staveb jsou převzaté z jednotlivých objektů OSM. U 243 katastrálních půdorysů se podařilo spojit údaj o podlažích pomocí identifikátoru stavebního objektu. Pekařova brána stojí na středu úseku OSM `way/359163465` a natočení průjezdu odpovídá tomuto úseku. V `data.js` zůstávají jejich identifikátory a typy, stejně jako identifikátory převzatých komunikací a vodních toků.
- Název Čtvrt je na [mapě ČÚZK](https://openzu.cuzk.gov.cz/opendata/MORP50/Turnov.pdf), zatímco [studie krajiny](https://www.krapr-naki.cz/file/22/orp-turnov_textova_cast-def.pdf) používá Čtvrť. V seznamu uvádíme obě podoby k místnímu ověření. Hůra je místní označení vrchu Vyskeř.
- [Pekařova brána](https://www.cesky-raj.info/cile/vysker) je stylizovaná pískovcová skála s lomeným průjezdem. Pro viditelnost je velikost výrazně zvětšena, okolní terén je zjednodušený a nejde o detailní rekonstrukci. Fotografie slouží pouze jako reference a nejsou součástí modelu.
- [Popis kaple](https://www.cesky-raj.info/vysker-kaple) a [NPÚ](https://www.npu.cz/cs/uop-liberec/prezentace-pamatek/vysker-kaple-sv-anny) slouží jako architektonická reference. V modelu nejsou použity jejich fotografie. Náhradní zobrazení používá stávající ilustraci webu `public/images/chapel-hura-light.webp` a její tmavou variantu. Jejich původ je vhodné před publikací zkontrolovat v projektových podkladech.

[ČÚZK poskytuje tato otevřená prostorová data pod CC BY 4.0](https://cuzk.gov.cz/Uvod/Produkty-a-sluzby/Otevrena-data/Otevrena-data-zakladni-informace.aspx). Upravené výřezy uvádějí zdroj a změny. Data OpenStreetMap jsou pod [ODbL](https://opendatacommons.org/licenses/odbl/1-0/), s uvedením © přispěvatelé OpenStreetMap na stránce. Při další distribuci odvozené databáze je třeba dodržet podmínky ODbL. Three.js a OrbitControls mají přiloženou licenci MIT.

## Obnovení mapových dat

Ve zvláštním prostředí Pythonu s balíčky `pyshp`, `pyproj` a `shapely` stáhněte oba uvedené ZIP soubory a výřez OSM XML, poté spusťte:

```sh
python scripts/relief/prepare_data.py --ruian 577685.zip --cadastre 787744.zip --osm vysker-osm.xml
```

Výsledný `public/relief/data.js` je zjednodušený výřez pro prohlížeč. Převod mění souřadnice S-JTSK na WGS 84. Pro přesné srovnání je nutné počítat s odchylkou transformace a s interpolací reliéfu. Data se nenačítají z cizí služby při návštěvě stránky.

## Další práce před zveřejněním

Prověřit aktuálnost vrstev, původ a licenci konkrétního výškového výřezu, přesnou polohu a orientaci kaple, doplnit vzhled dalších dominant podle doložených podkladů a vyzkoušet model na skutečných telefonech a s asistenčními technologiemi. Vizualizace cest, vodních toků, lesů, střech i oken je schematická. Věž kostela je velikostí zvýrazněný orientační model, nikoli přesná rekonstrukce. Kvalita a rozsah dat závisí na příslušných zdrojích.

## Barevné komunikace a fakta

Oranžové silnice jsou podle klasifikace OpenStreetMap silnice III. třídy, fialové čáry představují typy `residential`, `unclassified` a `living_street`. Šedě jsou vykreslené příjezdové, polní a pěší cesty. Jde o orientační mapové rozlišení, nikoliv doklad právního zařazení, správy nebo vlastnictví konkrétního úseku. Výběr místa zobrazí ověřený stručný údaj se zdrojem, případně pouze jeho zdrojovou mapovou polohu. Dřevěná zvonice je stylizovaná podle polohy OSM `way/369866838`.

## Pěší turistické trasy

Z výřezu OpenStreetMap jsou převzaté úseky relací pěších tras s atributy `kct_red`, `kct_blue`, `kct_green` a `kct_yellow`, pro hodnoty pásového značení a vyznačených odboček. Výsledek je oříznutý na terénní výřez a odvozený jen z dostupných zakreslených cest. Naučné stezky s jiným symbolem, cyklotrasy a neznačené či chybějící úseky nejsou v této vrstvě zahrnuté. Shodné barevné značky na stejném úseku jsou sloučené. Jednotlivá trasa leží ve středu zakreslené cesty, při souběhu se různé barvy kvůli čitelnosti nepatrně odsadí vedle sebe. Jemné čáry se vzorkují podle trojúhelníků zobrazeného terénu a bílé okraje je oddělují od barev komunikací. Poloha vychází ze zjednodušené geometrie OSM a zůstává orientační.

V `data.js` jsou uchovány identifikátory OSM cest a relací, jejich barvy a případná referenční čísla. Uživatel může vrstvu zapnout nebo vypnout. Značení se může měnit, proto je před publikací vhodné zkontrolovat aktuálnost a soulad s místním značením. [Popis turistického značení KČT](https://kct.cz/turisticka-znacka-po-znacenych-tras-klubu-ceskych-turistu-dojdete-k-cili-nejlepe/).

## Vložení do stránky

V editoru stránky lze použít tlačítko **Vložit 3D reliéf**. Vloží značku `{{relief}}` na samostatný odstavec, oddělený prázdnými řádky. Stejnou značku lze zapsat i ručně. Na jednu stránku se vloží jeden model. V náhledu administrace se místo modelu zobrazí informace o jeho umístění a odkaz na samostatnou stránku. Značka v ukázce kódu, seznamu nebo citaci zůstane běžným textem.
