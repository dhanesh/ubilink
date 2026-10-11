# Ubilink

Generated from the current project, including edits awaiting autosave. Return to the [theme index](../themes.md). Install the selected fonts and icons using the setup below; assets are acquired separately from their official sources.

## Font and icon setup

Follow [asset installation](../assets.md) when implementing this theme. Download and configure the selected free assets from their official sources; no font files or icon artwork are bundled. Weights below are the authored requests: load matching static faces or a variable range and report any unavailable weight. Register the exact family aliases below, or map the role tokens to the loader’s actual family.

| Selected font | Roles | Weights | Family stack | Setup |
| --- | --- | --- | --- | --- |
| <code>Inter</code> (free) | brand, ui, editorial, data | 500, 600 | <code>"Sidebar Inter", sans-serif</code> | Download and load [Inter](https://rsms.me/inter/) in the app. |

### Icons

Selected: [Lucide](https://lucide.dev/guide/react/getting-started), outlined. Install and wire the selected free pack (React: <code>lucide-react</code>); reuse it if already installed. Use the official adapter for other frameworks.

Preserve icon size, color, weight, style, and accessible names. Match the selected style only where the pack supports it; Lucide’s official pack supplies outlines, so report a filled-style gap rather than claiming exact filled artwork. Keep existing package versions and add only missing packages using the receiving project’s package manager.

## Foundations

```json
{
  "name": "Ubilink",
  "text": {
    "l": {
      "size": 21,
      "lineHeight": 28
    },
    "m": {
      "size": 14,
      "lineHeight": 21
    },
    "s": {
      "size": 12,
      "lineHeight": 18
    },
    "xl": {
      "size": 32,
      "lineHeight": 36
    },
    "xs": {
      "size": 11,
      "lineHeight": 14
    },
    "xxl": {
      "size": 43,
      "lineHeight": 46
    },
    "xxs": {
      "size": 9,
      "lineHeight": 12
    }
  },
  "fonts": {
    "ui": {
      "family": "\"Sidebar Inter\", sans-serif",
      "weights": {
        "regular": 500,
        "medium": 600,
        "heavy": 600
      }
    },
    "brand": {
      "family": "\"Sidebar Inter\", sans-serif",
      "weights": {
        "regular": 500,
        "medium": 600,
        "heavy": 600
      }
    },
    "editorial": {
      "family": "\"Sidebar Inter\", sans-serif",
      "weights": {
        "regular": 500,
        "medium": 600,
        "heavy": 600
      }
    },
    "data": {
      "family": "\"Sidebar Inter\", sans-serif",
      "weights": {
        "regular": 500,
        "medium": 600,
        "heavy": 600
      }
    }
  },
  "border": {
    "l": 0,
    "m": 0,
    "s": 0,
    "none": 0
  },
  "radius": {
    "l": 20,
    "m": 15,
    "s": 11,
    "xl": 28,
    "xs": 6,
    "full": 9999,
    "zero": 0
  },
  "shadows": {
    "l": {
      "x": 0,
      "y": 16,
      "blur": 48,
      "color": {
        "dark": "neutral-1",
        "light": "neutral-10"
      },
      "spread": 0,
      "opacity": 0
    },
    "m": {
      "x": 0,
      "y": 8,
      "blur": 24,
      "color": {
        "dark": "neutral-1",
        "light": "neutral-10"
      },
      "spread": 0,
      "opacity": 12
    },
    "s": {
      "x": 0,
      "y": 2,
      "blur": 4,
      "color": {
        "dark": "neutral-1",
        "light": "neutral-10"
      },
      "spread": 0,
      "opacity": 0
    }
  },
  "spacing": {
    "l": 17,
    "m": 12,
    "s": 9,
    "xl": 23,
    "xs": 6,
    "xxl": 35,
    "xxs": 3,
    "zero": 0
  },
  "animation": {
    "large": {
      "type": "easing",
      "duration": 280,
      "easing": [
        0.16,
        1,
        0.3,
        1
      ],
      "visualDuration": 0.36,
      "bounce": 0.3
    },
    "easing": [
      0.16,
      1,
      0.3,
      1
    ],
    "duration": 160,
    "popupScale": 0.96,
    "pressDistance": 1,
    "type": "easing",
    "visualDuration": 0.2,
    "bounce": 0.2
  },
  "iconStyle": "outlined",
  "iconFamily": "Lucide",
  "neutralTone": "warm",
  "buttonRadius": "s",
  "colorEmphasis": 87.82877604166667,
  "letterSpacing": {
    "l": 0.01,
    "m": 0,
    "s": -0.01,
    "xl": 0.02,
    "xs": -0.02
  },
  "primaryForeground": {
    "dark": "neutral-2",
    "light": "neutral-2"
  },
  "primaryActionColor": "neutral-10"
}
```

## light CSS variables

Define these in the app’s existing theme scope for this mode. Keep component styles linked to the variables.

| Variable | Value |
| --- | --- |
| `--theme-name` | Ubilink |
| `--theme-icon-family` | Lucide |
| `--theme-icon-style` | outlined |
| `--toolbar-divider-bleed` | 0 |
| `--focus-ring-outline` | initial |
| `--icon-stroke-width` | 2.5 |
| `--icon-light-display` | none |
| `--icon-regular-display` | none |
| `--icon-bold-display` | inline |
| `--motion-duration` | 160ms |
| `--motion-easing` | cubic-bezier(0.16, 1, 0.3, 1) |
| `--motion-type` | easing |
| `--motion-visual-duration` | 0.16 |
| `--motion-bounce` | 0.2 |
| `--motion-enabled` | 1 |
| `--motion-small-iterations` | infinite |
| `--motion-large-duration` | 280ms |
| `--motion-large-easing` | cubic-bezier(0.16, 1, 0.3, 1) |
| `--motion-large-type` | easing |
| `--motion-large-visual-duration` | 0.28 |
| `--motion-large-bounce` | 0.3 |
| `--motion-large-iterations` | infinite |
| `--motion-popup-scale` | 0.96 |
| `--motion-press-distance` | 1px |
| `--option-badge-background` | #fc032d |
| `--option-badge-foreground` | #fcfbfa |
| `--navigation-active-foreground` | #fcfbfa |
| `--emphasis-chart-fill` | #fc032d33 |
| `--emphasis-balance-background` | #ffd7ad |
| `--emphasis-rewards-background` | #f5f4f1 |
| `--emphasis-icon-background` | #f5f4f1 |
| `--emphasis-icon-foreground` | #000000 |
| `--emphasis-type-background` | #f5f4f1 |
| `--emphasis-type-foreground` | #000000 |
| `--navigation-active-background` | #fc032d |
| `--surface-raised-image` | none |
| `--surface-raised-shadow` | 0 0 0 0 transparent |
| `--surface-recessed-image` | none |
| `--surface-recessed-shadow` | 0 0 0 0 transparent |
| `--space-zero` | 0px |
| `--space-xxs` | 3px |
| `--space-xs` | 6px |
| `--space-s` | 9px |
| `--space-m` | 12px |
| `--space-l` | 17px |
| `--space-xl` | 23px |
| `--space-xxl` | 35px |
| `--tracking-xs` | -0.02em |
| `--tracking-s` | -0.01em |
| `--tracking-m` | 0em |
| `--tracking-l` | 0.01em |
| `--tracking-xl` | 0.02em |
| `--size-xxs` | 9px |
| `--line-xxs` | 12px |
| `--letter-spacing-xxs` | 0.02em |
| `--size-xs` | 11px |
| `--line-xs` | 14px |
| `--letter-spacing-xs` | 0.01em |
| `--size-s` | 12px |
| `--line-s` | 18px |
| `--letter-spacing-s` | 0.01em |
| `--size-m` | 14px |
| `--line-m` | 21px |
| `--letter-spacing-m` | 0em |
| `--size-l` | 21px |
| `--line-l` | 28px |
| `--letter-spacing-l` | -0.01em |
| `--size-xl` | 32px |
| `--line-xl` | 36px |
| `--letter-spacing-xl` | -0.01em |
| `--size-xxl` | 43px |
| `--line-xxl` | 46px |
| `--letter-spacing-xxl` | -0.02em |
| `--radius-zero` | 0px |
| `--radius-xs` | 6px |
| `--radius-s` | 11px |
| `--radius-m` | 15px |
| `--radius-l` | 20px |
| `--radius-xl` | 28px |
| `--radius-full` | 9999px |
| `--border-none` | 0px |
| `--border-s` | 0px |
| `--border-m` | 0px |
| `--border-l` | 0px |
| `--border-default-color` | #eae7e133 |
| `--border-shadow-none` | 0 0 0 0 transparent |
| `--border-shadow-s` | 0 0 0 0 transparent |
| `--border-shadow-m` | 0 0 0 0 transparent |
| `--border-shadow-l` | 0 0 0 0 transparent |
| `--font-ui` | "Sidebar Inter", sans-serif |
| `--weight-ui-regular` | 500 |
| `--weight-ui-medium` | 600 |
| `--weight-ui-heavy` | 600 |
| `--font-brand` | "Sidebar Inter", sans-serif |
| `--weight-brand-regular` | 500 |
| `--weight-brand-medium` | 600 |
| `--weight-brand-heavy` | 600 |
| `--font-editorial` | "Sidebar Inter", sans-serif |
| `--weight-editorial-regular` | 500 |
| `--weight-editorial-medium` | 600 |
| `--weight-editorial-heavy` | 600 |
| `--font-data` | "Sidebar Inter", sans-serif |
| `--weight-data-regular` | 500 |
| `--weight-data-medium` | 600 |
| `--weight-data-heavy` | 600 |
| `--color-none` | transparent |
| `--color-1` | #fc032d |
| `--color-1-transparent` | #fc032d33 |
| `--color-2` | #ffc5ce |
| `--color-2-transparent` | #ffc5ce33 |
| `--color-3` | #153d85 |
| `--color-3-transparent` | #153d8533 |
| `--color-4` | #ffd7ad |
| `--color-4-transparent` | #ffd7ad33 |
| `--neutral-1` | #ffffff |
| `--neutral-1-transparent` | #ffffff33 |
| `--neutral-2` | #fcfbfa |
| `--neutral-2-transparent` | #fcfbfa33 |
| `--neutral-3` | #f5f4f1 |
| `--neutral-3-transparent` | #f5f4f133 |
| `--neutral-4` | #eae7e1 |
| `--neutral-4-transparent` | #eae7e133 |
| `--neutral-5` | #d3cdc0 |
| `--neutral-5-transparent` | #d3cdc033 |
| `--neutral-6` | #a6a195 |
| `--neutral-6-transparent` | #a6a19533 |
| `--neutral-7` | #827d71 |
| `--neutral-7-transparent` | #827d7133 |
| `--neutral-8` | #5a554a |
| `--neutral-8-transparent` | #5a554a33 |
| `--neutral-9` | #383329 |
| `--neutral-9-transparent` | #38332933 |
| `--neutral-10` | #000000 |
| `--neutral-10-transparent` | #00000033 |
| `--success` | #00906c |
| `--success-transparent` | #00906c33 |
| `--warning` | #ffea00 |
| `--warning-transparent` | #ffea0033 |
| `--error` | #fc032d |
| `--error-transparent` | #fc032d33 |
| `--shadow-none` | none |
| `--shadow-s` | 0px 2px 4px 0px #00000000 |
| `--shadow-m` | 0px 2px 6px 0px #0000000d, 0px 8px 24px 0px #00000013 |
| `--shadow-l` | 0px 16px 48px 0px #00000000 |
| `--cte-canvas` | #ffffff |
| `--cte-surface` | #fcfbfa |
| `--cte-surface-muted` | #f5f4f1 |
| `--cte-text` | #000000 |
| `--cte-text-muted` | #827d71 |
| `--cte-border` | #eae7e133 |
| `--cte-accent` | #fc032d |
| `--cte-accent-text` | #fcfbfa |
| `--cte-danger` | #fc032d |
| `--cte-focus` | #fc032d |
| `--cte-font` | "Sidebar Inter", sans-serif |
| `--cte-font-size` | 12px |
| `--cte-font-weight` | 500 |
| `--cte-line-height` | 18px |
| `--cte-letter-spacing` | 0.01em |
| `--cte-detail-font-size` | 11px |
| `--cte-detail-line-height` | 14px |
| `--cte-detail-letter-spacing` | 0.01em |

## dark CSS variables

Define these in the app’s existing theme scope for this mode. Keep component styles linked to the variables.

| Variable | Value |
| --- | --- |
| `--theme-name` | Ubilink |
| `--theme-icon-family` | Lucide |
| `--theme-icon-style` | outlined |
| `--toolbar-divider-bleed` | 0 |
| `--focus-ring-outline` | initial |
| `--icon-stroke-width` | 2.5 |
| `--icon-light-display` | none |
| `--icon-regular-display` | none |
| `--icon-bold-display` | inline |
| `--motion-duration` | 160ms |
| `--motion-easing` | cubic-bezier(0.16, 1, 0.3, 1) |
| `--motion-type` | easing |
| `--motion-visual-duration` | 0.16 |
| `--motion-bounce` | 0.2 |
| `--motion-enabled` | 1 |
| `--motion-small-iterations` | infinite |
| `--motion-large-duration` | 280ms |
| `--motion-large-easing` | cubic-bezier(0.16, 1, 0.3, 1) |
| `--motion-large-type` | easing |
| `--motion-large-visual-duration` | 0.28 |
| `--motion-large-bounce` | 0.3 |
| `--motion-large-iterations` | infinite |
| `--motion-popup-scale` | 0.96 |
| `--motion-press-distance` | 1px |
| `--option-badge-background` | #fc032d |
| `--option-badge-foreground` | #fcfbfa |
| `--navigation-active-foreground` | #fcfbfa |
| `--emphasis-chart-fill` | #fc032d33 |
| `--emphasis-balance-background` | #ffd7ad |
| `--emphasis-rewards-background` | #2f2a21 |
| `--emphasis-icon-background` | #2f2a21 |
| `--emphasis-icon-foreground` | #ffffff |
| `--emphasis-type-background` | #2f2a21 |
| `--emphasis-type-foreground` | #ffffff |
| `--navigation-active-background` | #fc032d |
| `--surface-raised-image` | none |
| `--surface-raised-shadow` | 0 0 0 0 transparent |
| `--surface-recessed-image` | none |
| `--surface-recessed-shadow` | 0 0 0 0 transparent |
| `--space-zero` | 0px |
| `--space-xxs` | 3px |
| `--space-xs` | 6px |
| `--space-s` | 9px |
| `--space-m` | 12px |
| `--space-l` | 17px |
| `--space-xl` | 23px |
| `--space-xxl` | 35px |
| `--tracking-xs` | -0.02em |
| `--tracking-s` | -0.01em |
| `--tracking-m` | 0em |
| `--tracking-l` | 0.01em |
| `--tracking-xl` | 0.02em |
| `--size-xxs` | 9px |
| `--line-xxs` | 12px |
| `--letter-spacing-xxs` | 0.02em |
| `--size-xs` | 11px |
| `--line-xs` | 14px |
| `--letter-spacing-xs` | 0.01em |
| `--size-s` | 12px |
| `--line-s` | 18px |
| `--letter-spacing-s` | 0.01em |
| `--size-m` | 14px |
| `--line-m` | 21px |
| `--letter-spacing-m` | 0em |
| `--size-l` | 21px |
| `--line-l` | 28px |
| `--letter-spacing-l` | -0.01em |
| `--size-xl` | 32px |
| `--line-xl` | 36px |
| `--letter-spacing-xl` | -0.01em |
| `--size-xxl` | 43px |
| `--line-xxl` | 46px |
| `--letter-spacing-xxl` | -0.02em |
| `--radius-zero` | 0px |
| `--radius-xs` | 6px |
| `--radius-s` | 11px |
| `--radius-m` | 15px |
| `--radius-l` | 20px |
| `--radius-xl` | 28px |
| `--radius-full` | 9999px |
| `--border-none` | 0px |
| `--border-s` | 0px |
| `--border-m` | 0px |
| `--border-l` | 0px |
| `--border-default-color` | #423d3333 |
| `--border-shadow-none` | 0 0 0 0 transparent |
| `--border-shadow-s` | 0 0 0 0 transparent |
| `--border-shadow-m` | 0 0 0 0 transparent |
| `--border-shadow-l` | 0 0 0 0 transparent |
| `--font-ui` | "Sidebar Inter", sans-serif |
| `--weight-ui-regular` | 500 |
| `--weight-ui-medium` | 600 |
| `--weight-ui-heavy` | 600 |
| `--font-brand` | "Sidebar Inter", sans-serif |
| `--weight-brand-regular` | 500 |
| `--weight-brand-medium` | 600 |
| `--weight-brand-heavy` | 600 |
| `--font-editorial` | "Sidebar Inter", sans-serif |
| `--weight-editorial-regular` | 500 |
| `--weight-editorial-medium` | 600 |
| `--weight-editorial-heavy` | 600 |
| `--font-data` | "Sidebar Inter", sans-serif |
| `--weight-data-regular` | 500 |
| `--weight-data-medium` | 600 |
| `--weight-data-heavy` | 600 |
| `--color-none` | transparent |
| `--color-1` | #fc032d |
| `--color-1-transparent` | #fc032d33 |
| `--color-2` | #ffc5ce |
| `--color-2-transparent` | #ffc5ce33 |
| `--color-3` | #153d85 |
| `--color-3-transparent` | #153d8533 |
| `--color-4` | #ffd7ad |
| `--color-4-transparent` | #ffd7ad33 |
| `--neutral-1` | #000000 |
| `--neutral-1-transparent` | #00000033 |
| `--neutral-2` | #241f16 |
| `--neutral-2-transparent` | #241f1633 |
| `--neutral-3` | #2f2a21 |
| `--neutral-3-transparent` | #2f2a2133 |
| `--neutral-4` | #423d33 |
| `--neutral-4-transparent` | #423d3333 |
| `--neutral-5` | #5b564b |
| `--neutral-5-transparent` | #5b564b33 |
| `--neutral-6` | #888377 |
| `--neutral-6-transparent` | #88837733 |
| `--neutral-7` | #b1ab9f |
| `--neutral-7-transparent` | #b1ab9f33 |
| `--neutral-8` | #d5cfc3 |
| `--neutral-8-transparent` | #d5cfc333 |
| `--neutral-9` | #f5f4f1 |
| `--neutral-9-transparent` | #f5f4f133 |
| `--neutral-10` | #ffffff |
| `--neutral-10-transparent` | #ffffff33 |
| `--success` | #00906c |
| `--success-transparent` | #00906c33 |
| `--warning` | #ffea00 |
| `--warning-transparent` | #ffea0033 |
| `--error` | #fc032d |
| `--error-transparent` | #fc032d33 |
| `--shadow-none` | none |
| `--shadow-s` | 0px 2px 4px 0px #00000000 |
| `--shadow-m` | 0px 2px 6px 0px #0000000d, 0px 8px 24px 0px #00000013 |
| `--shadow-l` | 0px 16px 48px 0px #00000000 |
| `--cte-canvas` | #000000 |
| `--cte-surface` | #241f16 |
| `--cte-surface-muted` | #2f2a21 |
| `--cte-text` | #ffffff |
| `--cte-text-muted` | #b1ab9f |
| `--cte-border` | #423d3333 |
| `--cte-accent` | #fc032d |
| `--cte-accent-text` | #241f16 |
| `--cte-danger` | #fc032d |
| `--cte-focus` | #fc032d |
| `--cte-font` | "Sidebar Inter", sans-serif |
| `--cte-font-size` | 12px |
| `--cte-font-weight` | 500 |
| `--cte-line-height` | 18px |
| `--cte-letter-spacing` | 0.01em |
| `--cte-detail-font-size` | 11px |
| `--cte-detail-line-height` | 14px |
| `--cte-detail-letter-spacing` | 0.01em |

## Authored component assignments

These are project edits. The [component reference](ubilink-components.md) includes the effective assignments with defaults and shared parts resolved.

```json
{
  "componentTokens": {
    "button:ghost:rest": {
      "paddingX": "l",
      "paddingTop": "s",
      "paddingBottom": "s"
    },
    "button:danger:rest": {
      "paddingX": "l",
      "paddingTop": "s",
      "paddingBottom": "s"
    },
    "input:default:rest": {
      "paddingX": "m",
      "background": "neutral-3",
      "paddingTop": "s",
      "paddingBottom": "s"
    },
    "button:outline:rest": {
      "paddingX": "l",
      "paddingTop": "s",
      "paddingBottom": "s"
    },
    "button:primary:rest": {
      "paddingX": "l",
      "paddingTop": "s",
      "paddingBottom": "s"
    },
    "select:default:rest": {
      "paddingX": "m",
      "background": "neutral-3",
      "paddingTop": "s",
      "paddingBottom": "s"
    },
    "button:secondary:rest": {
      "paddingX": "l",
      "paddingTop": "s",
      "paddingBottom": "s"
    },
    "combobox:default:rest": {
      "paddingX": "m",
      "background": "neutral-3",
      "paddingTop": "s",
      "paddingBottom": "s"
    },
    "menu:default:part:option:rest": {
      "paddingX": "xs",
      "paddingTop": "xs",
      "paddingLeft": "xs",
      "paddingRight": "xs",
      "paddingBottom": "xs"
    },
    "slider:default:part:thumb:rest": {
      "controlSize": "l"
    },
    "slider:default:part:track:rest": {
      "controlSize": "xl"
    },
    "switch:default:part:control:rest": {
      "controlSize": "xl"
    },
    "combobox:default:part:option:rest": {
      "paddingX": "xs",
      "paddingTop": "xs",
      "paddingLeft": "xs",
      "paddingRight": "xs",
      "paddingBottom": "xs"
    },
    "menu:default:part:option:selected": {
      "paddingX": "xs",
      "paddingTop": "xs",
      "paddingLeft": "xs",
      "paddingRight": "xs",
      "paddingBottom": "xs"
    },
    "otp-field:default:part:input:rest": {
      "paddingX": "s",
      "paddingTop": "xs",
      "paddingLeft": "s",
      "paddingRight": "s",
      "paddingBottom": "xs"
    },
    "checkbox:default:part:control:rest": {
      "controlSize": "l"
    },
    "autocomplete:default:part:input:rest": {
      "paddingX": "s",
      "paddingTop": "s",
      "paddingLeft": "s",
      "paddingRight": "s",
      "paddingBottom": "s"
    },
    "autocomplete:default:part:option:rest": {
      "paddingX": "xs",
      "paddingTop": "xs",
      "paddingLeft": "xs",
      "paddingRight": "xs",
      "paddingBottom": "xs"
    },
    "combobox:default:part:option:selected": {
      "paddingX": "xs",
      "paddingTop": "xs",
      "paddingLeft": "xs",
      "paddingRight": "xs",
      "paddingBottom": "xs"
    },
    "autocomplete:default:part:popover:rest": {
      "radius": "s"
    },
    "autocomplete:default:part:option:selected": {
      "paddingX": "xs",
      "paddingTop": "xs",
      "paddingLeft": "xs",
      "paddingRight": "xs",
      "paddingBottom": "xs"
    }
  },
  "componentVariants": {}
}
```
