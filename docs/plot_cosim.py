"""Gráficas de la co-simulación semáforo (SE) + botón (CS).

Sigue el método de la skill `dataviz`:
  · Forma: cambio-en-el-tiempo -> escalones. Tres señales de naturaleza distinta
    (fase binaria, botón binario, sigma continua) -> SMALL MULTIPLES apilados,
    NUNCA dos escalas Y en el mismo eje.
  · Color por trabajo: la fase es 'status' (rojo/verde semánticos del semáforo).
  · El par rojo/verde es el caso clásico de colisión para daltonismo, así que
    lleva CODIFICACIÓN SECUNDARIA obligatoria: etiqueta de texto dentro de cada
    banda + trama (hatch) distinta. La identidad nunca depende solo del color.
  · Escalones `steps-post`: sigma es una ESCALERA, no una rampa.
"""
import sys
import numpy as np
import pandas as pd
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import Patch

CSV = sys.argv[1]
OUT_LIGHT = sys.argv[2]
OUT_DARK = sys.argv[3]

# ── Paleta (valores de references/palette.md, ya validados) ───────────────────
# El modo oscuro NO es un volcado del claro: usa los pasos de la columna "dark"
# de la paleta. (Rojo status y verde categórico son iguales en ambos modos;
# azul y naranja tienen paso propio.)
RED     = "#d03b3b"   # status critical  -> fase ROJA (igual en ambos modos)
GREEN   = "#008300"   # categorical 6    -> fase VERDE (igual en ambos modos)
BLUE    = {"light": "#2a78d6", "dark": "#3987e5"}   # categorical 1 -> botón
ORANGE  = {"light": "#eb6834", "dark": "#d95926"}   # categorical 2 -> sigma

INK      = {"light": "#0b0b0b", "dark": "#ffffff"}
INK2     = {"light": "#52514e", "dark": "#c3c2b7"}
MUTED    = "#898781"
GRID     = {"light": "#e1e0d9", "dark": "#2c2c2a"}
SURFACE  = {"light": "#fcfcfb", "dark": "#1a1a19"}


# ── Comprobación CVD (no “a ojo”: se calcula) ─────────────────────────────────
def _srgb_to_linear(c):
    c = np.asarray(c, dtype=float)
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def _hex_to_rgb(h):
    h = h.lstrip("#")
    return np.array([int(h[i:i + 2], 16) / 255 for i in (0, 2, 4)])


def _linear_to_oklab(lin):
    M1 = np.array([[0.4122214708, 0.5363325363, 0.0514459929],
                   [0.2119034982, 0.6806995451, 0.1073969566],
                   [0.0883024619, 0.2817188376, 0.6299787005]])
    lms = M1 @ lin
    lms = np.cbrt(np.clip(lms, 0, None))
    M2 = np.array([[0.2104542553, 0.7936177850, -0.0040720468],
                   [1.9779984951, -2.4285922050, 0.4505937099],
                   [0.0259040371, 0.7827717662, -0.8086757660]])
    return M2 @ lms


def _simulate_deuteranopia(lin):
    # Matriz clásica (Viénot) aplicada en RGB lineal.
    M = np.array([[0.625, 0.375, 0.0],
                  [0.700, 0.300, 0.0],
                  [0.000, 0.300, 0.700]])
    return M @ lin


def delta_e_ok(h1, h2, cvd=False):
    a, b = _srgb_to_linear(_hex_to_rgb(h1)), _srgb_to_linear(_hex_to_rgb(h2))
    if cvd:
        a, b = _simulate_deuteranopia(a), _simulate_deuteranopia(b)
    return float(np.linalg.norm(_linear_to_oklab(a) - _linear_to_oklab(b)) * 100)


print("── Comprobación de color (calculada, no estimada) ──")
for n1, c1, n2, c2 in [("ROJO", RED, "VERDE", GREEN),
                       ("ROJO", RED, "botón", BLUE["light"]),
                       ("VERDE", GREEN, "sigma", ORANGE["light"])]:
    normal, cvd = delta_e_ok(c1, c2), delta_e_ok(c1, c2, cvd=True)
    flag = "OK" if cvd >= 8 else ("necesita codificación secundaria" if cvd >= 6 else "FALLA sin codificación secundaria")
    print(f"  {n1:5s} vs {n2:6s}: ΔE normal={normal:5.1f}  ΔE deuteranopía={cvd:5.1f}  -> {flag}")
print("  -> el par ROJO/VERDE lleva etiqueta de texto + trama en cada banda.\n")

# ── Datos ─────────────────────────────────────────────────────────────────────
df = pd.read_csv(CSV)
t = df.iloc[:, 0].to_numpy()
# El orquestador reemplaza espacios por "_", así que las etiquetas quedan
# como "semaforoV2_se_—_rojo": nos quedamos con lo que va tras el guión largo.
col = {c.split("—")[-1].strip("_ "): c for c in df.columns[1:]}
rojo = df[col["rojo"]].to_numpy()
verde = df[col["verde"]].to_numpy()
sigma = df[col["t_restante"]].to_numpy()
boton = df[col["salida"]].to_numpy()


def tramos(sig, t):
    """Intervalos [ini, fin) donde sig >= 0.5."""
    on = sig >= 0.5
    out, ini = [], None
    for i, v in enumerate(on):
        if v and ini is None:
            ini = t[i]
        elif not v and ini is not None:
            out.append((ini, t[i]))
            ini = None
    if ini is not None:
        out.append((ini, t[-1]))
    return out


tramos_rojo, tramos_verde = tramos(rojo, t), tramos(verde, t)
pulsos = tramos(boton, t)


def figura(modo, salida):
    ink, ink2, grid, surf = INK[modo], INK2[modo], GRID[modo], SURFACE[modo]
    blue, orange = BLUE[modo], ORANGE[modo]
    fig, axes = plt.subplots(3, 1, figsize=(13, 7.6), sharex=True,
                             height_ratios=[1.0, 1.0, 1.45])
    fig.patch.set_facecolor(surf)

    for ax in axes:
        ax.set_facecolor(surf)
        ax.grid(True, color=grid, linewidth=0.8, alpha=0.9)
        ax.set_axisbelow(True)
        for s in ax.spines.values():
            s.set_visible(False)
        ax.tick_params(colors=MUTED, labelsize=9)

    # ── Panel 1: fase del semáforo ───────────────────────────────────────────
    ax = axes[0]
    for (a, b), (color, nombre, hatch) in [(x, y) for x in tramos_rojo for y in [(RED, "ROJO", "///")]] + \
                                          [(x, y) for x in tramos_verde for y in [(GREEN, "VERDE", "\\\\\\")]]:
        ax.axvspan(a, b, color=color, alpha=0.92, hatch=hatch, edgecolor="none", linewidth=0)
        dur = b - a
        if dur > 8:  # etiqueta directa: la identidad NUNCA depende solo del color
            ax.text((a + b) / 2, 0.5, f"{nombre}\n{dur:.0f} s", ha="center", va="center",
                    color="white", fontsize=8.5, fontweight="bold", linespacing=1.3)
    ax.set_ylim(0, 1)
    ax.set_yticks([])
    ax.set_title("Fase del semáforo  ·  el botón acorta el rojo de 30 s a 20 s en total",
                 color=ink, fontsize=11, fontweight="bold", loc="left", pad=8)
    # Leyenda FUERA del área de datos: encima colisionaba con las bandas y
    # quedaba ilegible (tinta oscura sobre rojo/verde).
    ax.legend(handles=[Patch(facecolor=RED, hatch="///", label="Rojo"),
                       Patch(facecolor=GREEN, hatch="\\\\\\", label="Verde")],
              loc="lower right", bbox_to_anchor=(1.0, 1.005), frameon=False,
              fontsize=9, labelcolor=ink2, ncol=2, handlelength=1.6,
              handleheight=0.9, borderaxespad=0.0)

    # ── Panel 2: botón ───────────────────────────────────────────────────────
    ax = axes[1]
    ax.step(t, boton, where="post", color=blue, linewidth=2)
    ax.fill_between(t, 0, boton, step="post", color=blue, alpha=0.18, linewidth=0)
    for i, (a, _b) in enumerate(pulsos, 1):
        ax.annotate(f"#{i}\nt={a:.0f}", xy=(a, 1), xytext=(a, 1.45),
                    ha="center", fontsize=8, color=ink2,
                    arrowprops=dict(arrowstyle="-", color=MUTED, linewidth=0.8))
    ax.set_ylim(-0.1, 2.0)
    ax.set_yticks([0, 1])
    ax.set_ylabel("botón", color=ink2, fontsize=9)
    ax.set_title("Entrada: pulsaciones del peatón (FMU Co-Simulation)",
                 color=ink, fontsize=10, loc="left", pad=6)

    # ── Panel 3: sigma ───────────────────────────────────────────────────────
    ax = axes[2]
    ax.step(t, sigma, where="post", color=orange, linewidth=2)
    ax.set_ylabel("σ  [s]", color=ink2, fontsize=9)
    ax.set_xlabel("tiempo de simulación  [s]", color=ink2, fontsize=10)
    ax.set_title("σ = t_restante — el reloj countdown de la FMU (ta() de DEVS).  "
                 "Es una ESCALERA: solo cambia cuando se activa la partición.",
                 color=ink, fontsize=10, loc="left", pad=6)
    ax.set_ylim(0, 34)

    # Guías verticales en cada pulsación, en los tres paneles.
    for (a, _b) in pulsos:
        for axx in axes:
            axx.axvline(a, color=MUTED, linewidth=0.9, linestyle=(0, (4, 3)), alpha=0.8, zorder=0)

    axes[0].set_xlim(t[0], t[-1])
    fig.suptitle("Co-simulación FMI 3.0: semáforo aperiódico (Scheduled Execution) + botón (Co-Simulation)",
                 color=ink, fontsize=13, fontweight="bold", x=0.011, ha="left", y=0.985)
    fig.tight_layout(rect=[0, 0, 1, 0.955])
    fig.savefig(salida, dpi=160, facecolor=surf)
    plt.close(fig)
    print(f"  guardado: {salida}")


figura("light", OUT_LIGHT)
figura("dark", OUT_DARK)
