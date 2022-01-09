
# Eager Leader Election: Half-duplex Alternating

### Popis algoritmu

Eager (horlivý..?) algoritmus volby vůdce na kružnici v poloduplexní střídavé
variantě pracuje následujícím způsobem:

1. Paměť každého uzlu obsahuje unikátní ID a odkazy na levého a pravého souseda.
   Hodnoty těchto tří parametrů se v průběhu algoritmu nemění. Dále má uzel svůj
   *stav*, který se v průběhu algoritmu vyvíjí a může nabývat následujících
   hodnot:
  - `Candidate`: uzel kandiduje na vůdce a kromě konstantních parametrů má
    ještě parametry fáze algoritmu a číslo poslední fáze, ve které odesílal
    sondu.
  - `Defeated`: uzel již nemá šanci stát se vůdcem, kromě konstantních parametrů
    obsahuje volitelný parametr ID zvoleného vůdce.
  - `Leader`: tento uzel je zvoleným vůdcem bez dalších parametrů.
2. Algoritmus pokračuje v jednotlivých fázích:
  - v každé fázi vyšle každý kandidát jednu sondu (`ProbeMessage`) obsahující
    jeho ID, kterým směrem míří (doleva či doprava) a ve které fázi byla
    vyslána. V lichých fázích posílají kandidáti zprávy doprava, v sudých
    doleva.
    - nekandidující uzly sondy jen přeposílají stejným směrem, jakým byly
      vyslány
  - při přijetí sondy kandidát porovná své ID s ID odesílatele:
    - je-li jeho ID vyšší, postupuje do další fáze algoritmu
    - je-li jeho ID shodné s ID odesílatele, pak sonda oběhla kružnici a žádný
      kandidát ji nezastavil. Tento kandidát se proto stává vůdcem (jeho stav se
      mění na `Leader`) a vysílá zprávu `NotifyMessage` obsahující jeho ID a
      informaci o tom, kterým směrem míří. Touto zprávou ostatní uzly upozorní
      na fakt, že se stal vůdcem
    - je-li jeho ID nižší než ID odesílatele, je odesílatelem sondy poražen.
      Jeho stav se mění na `Defeated` a sondu přeposílá dál směrem, kterým byla
      vyslána
  - při přijetí `NotifyMessage` si poražený uzel uloží ID vůdce. Vůdce zprávu
    `NotifyMessage` ignoruje. Ve správné implementaci s **MO** (message
    ordering) kandidáti `NotifyMessage` neobdrží.

### Popis implementace

Implementace je založená na Rust knihovnách Tokio a Tonic a používá asynchronní
programování. Každý uzel je reprezentován serverovou službou, která odpovídá na
gRPC požadavky, a smyčkou iniciující rozesílání zpráv, která běží paralelně se
serverovou službou. Nejprve se načte seznam uzlů a vytvoří se serverová služba a
"klient" (smyčka pro odesílání sond) pro každé ID. Tyto se zabalí do `Future`
objektů a spustí zároveň, Tokio vytvoří systémová vlákna a rozloží výpočty mezi
ně.

Pro snadné odesílání zpráv je asynchronní API gRPC metod zabaleno do
synchronních funkcí `probe` a `notify_elected`. Protože výsledky gRPC metod jsou
nezajímavé – síťové chyby neošetřujeme a odpovědi nenesou žádnou hodnotu –
synchronní funkce spustí gRPC volání na pozadí a ihned vrátí kontrolu
volajícímu.

gRPC protokol definuje metody s obousměrným streamováním, které značně
komplikuje implementaci serveru. Většina logiky je obalená makrem `try_stream!`,
které umožňuje asynchronní ošetřování chyb. Komplikované typy kolem streamovací
podpory knihovny Tonic ale nakonec vedly k tomu, že jsem jejich možností v
implementaci nevyužil. Ve výsledku tak může dojít k tomu, že je průběh jednoho
volání gRPC metody vyhodnocen zároveň s jiným.

Klient je v implementaci potřeba k tomu, aby nějakou komunikaci mezi uzly
podnítil. Samotný server pouze reaguje na volání odjinud. Implementace klientské
smyčky je poměrně jednoduchá, obsahuje ale pokyn k uspání, aby nezačala odesílat
zprávy dříve, než se porty serverů otevřou. Protože je konceptuálně jediný uzel
rozdělen na klientskou a serverovou část, musí tyto dvě komponenty sdílet
modifikovatelný stav. Tento fakt vedl v implementaci k mnoha chybám a race
conditions. K synchronizaci se využívá asynchronní verze mutexu, která podporuje
zámky mutexu držené napříč `await` body (tedy místa, kde se vyhodnocování funkce
může přerušit).

V průběhu činnosti algoritmu se klientské smyčky vypínají jakmile je příslušný
uzel poražen. U serverů to ale není tak jednoduché, protože jsou zapotřebí k
přeposílání zpráv.

### Měření

Měření počtu zpráv najdete v souboru `measurements.csv`. Je také možné je
vygenerovat Python 3 skriptem `measure.py`. Jednoduchá vizualizce jejich
závislosti je zde:

![image](https://user-images.githubusercontent.com/7235381/148700040-64b2d035-a217-4cee-8be3-0079f2c9a2ba.png)
