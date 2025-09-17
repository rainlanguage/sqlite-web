import { Float } from '@rainlanguage/float';

export type PrefixedHex = `0x${string}`;

export function encodeFloatHex(decimal: string): PrefixedHex {
  const parseRes = Float.parse(decimal);
  if (parseRes.error) {
    throw new Error(`Float.parse failed: ${String(parseRes.error.msg ?? parseRes.error)}`);
  }
  return parseRes.value.asHex() as PrefixedHex;
}

function ensurePrefixedHex(hex: string): PrefixedHex {
  if (!hex.startsWith('0x')) {
    throw new Error(`Expected Float hex with 0x prefix, received: ${hex}`);
  }
  return hex as PrefixedHex;
}

export function decodeFloatHex(hex: PrefixedHex | string): string {
  const prefixed = ensurePrefixedHex(hex);
  const fromHexRes = Float.fromHex(prefixed);
  if (fromHexRes.error) {
    throw new Error(`Float.fromHex failed: ${String(fromHexRes.error.msg ?? fromHexRes.error)}`);
  }
  const formatRes = fromHexRes.value.format();
  if (formatRes.error) {
    throw new Error(`Float.format failed: ${String(formatRes.error.msg ?? formatRes.error)}`);
  }
  return formatRes.value as string;
}

export function withoutPrefix(hex: PrefixedHex): string {
  return hex.slice(2);
}

export function toMixedCase(hex: PrefixedHex): string {
  let result = '';
  for (let i = 0; i < hex.length; i++) {
    const char = hex[i] ?? '';
    if (/[a-f]/.test(char)) {
      result += i % 2 === 0 ? char.toUpperCase() : char.toLowerCase();
    } else {
      result += char;
    }
  }
  return result;
}

export function createFloatHexMap<T extends Record<string, string>>(decimals: T): { readonly [K in keyof T]: PrefixedHex } {
  const result: Partial<Record<keyof T, PrefixedHex>> = {};
  for (const [key, value] of Object.entries(decimals) as Array<[keyof T, string]>) {
    result[key] = encodeFloatHex(value);
  }
  return result as { readonly [K in keyof T]: PrefixedHex };
}
