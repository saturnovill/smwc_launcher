# smwc_launcher

Launcher estilo consola para descargar, parchear y jugar hacks de Super Mario World desde [SMWCentral](https://www.smwcentral.net/), con el emulador Snes9x integrado. Hecho en Rust con [egui](https://github.com/emilk/egui) y pensado para usarse 100% con mando (también funciona con ratón y teclado).

- Biblioteca de hacks descargados y búsqueda en SMWCentral con filtros (nombre, dificultad, tipo, orden).
- Descarga y parcheo automático (BPS / IPS) sobre tu ROM original de SMW (USA).
- Emulador Snes9x integrado (libretro) con menú de pausa y guardado de partidas (`.srm`).
- Interfaz en español e inglés.

## Compilar

Requiere [Rust](https://rustup.rs/).

```sh
cargo build --release
./target/release/smw-launcher
```

### Núcleo Snes9x

El emulador integrado usa el núcleo libretro de Snes9x. Para compilarlo (requiere `g++` y `make`):

```sh
git clone --depth 1 https://github.com/snes9xgit/snes9x
make -C snes9x/libretro -j"$(nproc)"
mkdir -p ~/.local/share/smw-launcher/cores
cp snes9x/libretro/snes9x_libretro.so ~/.local/share/smw-launcher/cores/
```

También se detecta en `/usr/lib/libretro/` (paquete `libretro-snes9x`) o en `~/.config/retroarch/cores/`, y se puede elegir otro núcleo o un emulador externo en **Ajustes**.

## Controles

| | Mando | Teclado |
|---|---|---|
| Moverse | Cruceta / stick | Flechas / Tab |
| Aceptar / Atrás | A / B | Enter / Esc |
| Cambiar pestaña | LB / RB | — |
| Pausa en el juego | HOME o SELECT+START | Esc |
| Botones SNES | Por posición | X=A, Z=B, S=X, A=Y, Q=L, W=R, Enter=Start, Retroceso=Select |
| Pantalla completa | — | F11 |

## Pruebas

```sh
cargo test
# Pruebas reales contra SMWCentral y el núcleo:
SMW_ROM=/ruta/smw.sfc SMW_CORE=/ruta/snes9x_libretro.so cargo test -- --ignored
```

## Créditos

- Basado en la idea de [SMWCentral-Downloader-Patcher](https://github.com/iamtheratio/SMWCentral-Downloader-Patcher).
- API de [SMWCentral](https://www.smwcentral.net/) (referencia: [smwc-api-python](https://github.com/R3tr0BoiDX/smwc-api-python)).
- Fuente [smw-font](https://github.com/vvye/smw-font) de wye, licencia [CC BY-NC 4.0](assets/FONT-LICENSE.md); convertida a TTF con `assets/font2ttf.py`.
- Emulador [Snes9x](https://github.com/snes9xgit/snes9x) (licencia propia, uso no comercial).

No se incluye ninguna ROM: necesitas tu propia copia de Super Mario World (USA).
