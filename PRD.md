Versión 0.2, ajustada para una primera validación técnica realizada por una sola persona, con 1 a 3 horas de trabajo por noche.

# PRD: Sistema de monitoreo personal inalámbrico (nombre provisional: _MixLink_)

## 1. Resumen

Un sistema de software que toma los canales de una mezcladora digital o interfaz de audio conectada a una computadora y entrega a cada músico, en su celular, una mezcla personal que él mismo controla. Está pensado para grupos de 10 o más músicos y para estudios, con apps nativas en iOS y Android.

## 2. Objetivos

- Que cada músico controle su mezcla (volumen, paneo y mute por canal) desde el celular.
- Latencia total de audio baja y estable, sin chasquidos.
- Soportar 10 a 16 músicos simultáneos desde un solo servidor.
- Configuración simple: sin pedir la IP a mano ni instalar drivers en el celular.

## 3. Fuera de alcance (v1)

- Procesamiento avanzado (EQ, compresión, reverb por músico).
- Control de la mezcla principal de la consola.
- Grabación multipista.
- Transmisión por internet; v1 funciona solo en red local.
- Escalado a 10-16 músicos antes de validar el flujo con 1-2 dispositivos.

## 4. Usuarios

- **Músico:** quiere escucharse bien y ajustar su mezcla sin molestar al ingeniero.
- **Ingeniero o técnico:** configura canales, nombres, grupos, límites y mezclas base, y supervisa la conexión de todos.
- **Administrador del estudio:** gestiona salas, presets e historial de sesiones.

## 5. Requerimientos funcionales

**Servidor (Rust + Tauri)**

- RF1. Seleccionar dispositivo de audio, frecuencia de muestreo y tamaño de buffer.
- RF2. Capturar hasta 32 canales de entrada.
- RF3. Generar una mezcla estéreo independiente por músico.
- RF4. Enviar el audio por UDP a cada cliente y recibir su control por un canal aparte.
- RF5. Mostrar medidores, estado de conexión y pérdida de paquetes por cliente.
- RF6. Descubrimiento automático (mDNS) y código QR para conectar.
- RF7. Guardar y cargar presets de sesión y de mezcla.
- RF12. Permitir al ingeniero bloquear canales y establecer límites de volumen por músico.

**Apps móviles (Swift y Kotlin)**

- RF8. Conectarse al servidor, elegir su perfil de músico y recibir el audio.
- RF9. Faders, paneo, mute y solo por canal, con grupos (voces, batería, etc.).
- RF10. Mezclas guardadas (bancos) y botón de "más de mí".
- RF11. Indicador de estabilidad y ajuste del buffer.

## 6. Requerimientos no funcionales

- **Latencia de extremo a extremo:** objetivo ≤ 25 ms, aceptable para MVP ≤ 40 ms.
- **Estabilidad:** sin cortes audibles en sesiones de 3 horas.
- **Ancho de banda aproximado** (48 kHz, estéreo, por músico): PCM 16 bits ≈ 1,5 Mbps; PCM 24 bits ≈ 2,3 Mbps; Opus ≈ 0,1 a 0,2 Mbps. Con 16 músicos, el PCM suma unos 25 a 37 Mbps. Con Opus, solo unos 2 a 3 Mbps.
- **Plataformas:** servidor en Windows y macOS (Linux opcional); iOS 16+ y Android 10+.
- **Privacidad:** sin recopilación de datos; todo funciona en red local.

## 7. Decisiones técnicas

- **Transporte:** UDP unicast con números de secuencia y buffer anticipado adaptativo.
- **Códec:** PCM en v1; Opus con tramas de 2,5 a 5 ms como opción para redes saturadas.
- **Audio móvil:** AVAudioEngine en iOS y Oboe/AAudio en Android.
- **Control:** WebSocket para faders, nombres y estado.
- **Red:** el cuello de botella con 10+ clientes es el WiFi, no la CPU. Se recomienda un router o AP dedicado en 5 GHz, y varios puntos de acceso para grupos grandes. La app debe avisar de redes inadecuadas.

## 8. Hitos

1. **M0 – Prueba de concepto:** captura desde la Volt 4 en Windows, mezcla estéreo básica, UDP a un iPhone o Android y medición de latencia y pérdida de paquetes.
2. **M1 – Mezclador y control:** mezclas por músico, canal de control, 4 clientes.
3. **M2 – Apps nativas:** interfaz completa en iOS y Android.
4. **M3 – Escala:** 10 a 16 clientes, Opus, métricas de estabilidad.
5. **M4 – Producto:** presets, instalador, documentación y beta con usuarios reales.

## 8.1 Plan de trabajo para M0

El objetivo de M0 es demostrar que el recorrido completo funciona antes de invertir en una interfaz móvil completa o en soporte multicanal.

1. **Preparar el entorno:** crear el proyecto Rust, seleccionar Windows como plataforma principal y comprobar que la Volt 4 aparece como dispositivo de entrada.
2. **Capturar audio:** leer audio PCM a 48 kHz, con tamaño de buffer configurable y registro de errores del dispositivo.
3. **Enviar audio:** empaquetar audio estéreo con números de secuencia y transmitirlo por UDP dentro de la red local.
4. **Reproducir en un cliente:** implementar un cliente de prueba para iPhone y Android, priorizando primero la plataforma que permita medir antes.
5. **Medir:** registrar latencia extremo a extremo, pérdida de paquetes, jitter y cortes audibles durante pruebas repetibles.
6. **Control mínimo:** agregar un canal de control para volumen, mute y límite de volumen del músico.
7. **Validar en macOS:** repetir la prueba básica con la Volt 4 y documentar diferencias respecto de Windows.
8. **Preparar el siguiente escenario:** probar la X32 con X-USB solo después de que M0 sea estable con la Volt 4.

### Criterios de aceptación de M0

- Un teléfono recibe y reproduce audio de la Volt 4 dentro de la misma red local.
- La latencia medida queda documentada, con objetivo de 25 ms o menos y límite de aceptación de 40 ms.
- La prueba informa pérdida de paquetes y cortes, en lugar de ocultarlos con reconexiones silenciosas.
- El ingeniero puede silenciar un canal y limitar el volumen máximo del músico.
- La prueba se puede repetir en Windows y queda preparada para una validación posterior en macOS.

## 9. Métricas de éxito

- Latencia medida de extremo a extremo.
- Pérdida de paquetes por cliente por debajo del 0,5 %.
- Tiempo de conexión de un músico nuevo por debajo de 30 s.
- Sesiones de 3 horas sin interrupciones con 10 clientes.

## 10. Riesgos

- Latencia o chasquidos por WiFi saturado (el riesgo más grande).
- Fragmentación de dispositivos Android, con latencias de audio muy distintas.
- Soporte de ASIO en Windows para interfaces multicanal.
- Alcance excesivo para un solo desarrollador; conviene validar primero con 1 o 2 clientes.
- Marca y diseño: crear una identidad propia, sin reutilizar nombre ni elementos de StageWave.

## 11. Preguntas abiertas

1. ¿El producto será de licencia única, suscripción o código abierto con versión de pago?
2. ¿Qué alcance tendrá la gestión de salas, presets e historial para la primera versión comercial?

Estas decisiones quedan fuera de M0 y se definirán antes de diseñar la primera versión comercial.
