/**
 * The look of a toolbar button, shared by the window's toolbar (`AppToolbar.vue`) and the merge
 * editor's (`ToolbarButton.vue`).
 */

/**
 * Flat toolbar button: icon on top, caption below, no fill or border at rest. Hover and press
 * lay the ink over the bar at 6% and 10%; disabled drops to 38% opacity and lets the pointer
 * through to the wrapper behind it, which carries the tooltip and the right-click menu. The 20px
 * icons draw a 1.67px line (Lucide's 2 units at 20px) where the app's other icons draw 1px.
 */
export const BUTTON_CLASS =
  "relative inline-flex h-[42px] w-[63px] shrink-0 flex-col items-center justify-between rounded-[4px] px-[2px] pt-[6px] pb-[5px] text-[10px] leading-none font-medium whitespace-nowrap transition-colors duration-[120ms] outline-none enabled:active:bg-foreground/10 focus-visible:ring-3 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-38 [&_svg]:size-5 [&_svg]:shrink-0 [&_svg]:[stroke-width:1.67px]";
/** A flat button's ink and hover. Kept off the primary button: `cn` does not take the primary
    fill's text colour for one that replaces `text-foreground`, so both stayed and the flat one won. */
export const FLAT_CLASS = "text-foreground/90 enabled:hover:bg-foreground/6 enabled:hover:text-foreground";
/** The primary button: solid primary fill, a step towards the primary colour on hover, darker pressed. */
export const PRIMARY_CLASS =
  "bg-primary-fill text-primary-fill-foreground enabled:hover:bg-[color-mix(in_srgb,var(--primary-fill)_40%,var(--primary))] enabled:hover:text-primary-fill-foreground enabled:active:bg-[color-mix(in_srgb,var(--primary-fill)_85%,black)]";

/** The caption under the icon, cut short to fit the button. */
export const CAPTION_CLASS = "block max-w-[59px] truncate text-[9px] font-medium tracking-[0.04em] uppercase";
