# octopus

Una app de bandeja del sistema que pega textos predefinidos (un email, un número de prueba) con un atajo de teclado global, en cualquier aplicación. Sin drivers ni permisos de teclado de bajo nivel.

- Los atajos se agrupan en **plantillas** (pestañas) y una sola está activa a la vez.
- Se configura en una ventana con una tabla: atajo, texto, descripción.
- Puede arrancar con el sistema.
- Hecha con Rust y [Tauri 2](https://tauri.app).

## Estado, con honestidad

- La escribió Claude (Claude Code) junto a una persona que usaba Rust por primera vez. **No la revisó ningún experto en Rust.**
- **Solo se probó abierta en macOS.** El CI compila el código, pasa `clippy` y corre los tests en macOS, Windows y Linux, pero nadie usó la app en Windows ni en Linux.
- **Linux:** los atajos globales dependen de X11. Ubuntu 25.10 y 26.04 solo ofrecen Wayland en GNOME, y según la documentación del crate de atajos no hay soporte ahí, así que en Ubuntu con la configuración por defecto probablemente no funcione.
- Los instaladores todavía no se generaron: `release.yml` no corrió.

## Compilar

```
cargo run                                 # desarrollo
cargo test
npx @tauri-apps/cli@2 build --bundles app # el .app de macOS, en target/release/bundle/macos/
```

En macOS, para pegar automáticamente hay que darle a la app el permiso de **Accesibilidad**. Sin él, el texto igual queda copiado y se pega a mano. `scripts/sign.sh` firma la app con un requisito estable para que macOS conserve el permiso entre compilaciones.

Los atajos se guardan en el archivo `octopus.json` de la carpeta de configuración del sistema (en macOS: `~/Library/Application Support/dev.octopus.app/`).

## Aprender con este código

Este repo también sirve como un mini curso de Rust: las guías explican qué hace cada archivo, por qué se decidió cada cosa así y los conceptos de Rust sobre código real, además de cómo funciona el CI/CD. **Leelas en <https://rpolverini.github.io/learning-with-octopus/>**.

El código fuente de las guías está en [learning-with-octopus](https://github.com/rpolverini/learning-with-octopus). Cuando cambia el código que ellas describen, un workflow abre una issue para revisarlas.

## Licencias

- **Código:** [GPL-3.0-or-later](LICENSE).
- **Arte** (`assets/`, `icons/`, `ui/octopus.png`): escultura en plastilina de **Luli Agüero**, con detalles agregados con herramientas de IA, bajo [CC BY-SA 4.0](assets/README.md).
