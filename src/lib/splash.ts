/**
 * Pure helpers and constants for the startup splash screen animation.
 */

/**
 * Duration in milliseconds that the startup splash overlay stays fully visible
 * before starting its fade-out transition.
 */
export const SPLASH_VISIBLE_MS = 1300;

/**
 * Duration in milliseconds for the splash overlay fade-out transition
 * before it is completely unmounted from the DOM.
 */
export const SPLASH_FADE_MS = 300;

/**
 * Determines whether the startup splash animation should be displayed.
 *
 * The splash is shown only once per app launch session and is completely bypassed
 * when the user has requested reduced motion for accessibility.
 *
 * @param reducedMotion - True if the user prefers reduced motion.
 * @param alreadyShown - True if the splash has already been shown in this session.
 * @returns True only when neither reducedMotion nor alreadyShown is true.
 */
export function shouldShowSplash(
  reducedMotion: boolean,
  alreadyShown: boolean
): boolean {
  return !reducedMotion && !alreadyShown;
}
