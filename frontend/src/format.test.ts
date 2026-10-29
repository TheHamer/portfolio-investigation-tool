import { describe, expect, it } from 'vitest';
import { formatGbp, formatQuantity, formatTimestamp, groupDigits, signClass } from './format';

describe('groupDigits', () => {
  it('inserts thousands separators into the whole part only', () => {
    expect(groupDigits('1234567.891')).toBe('1,234,567.891');
    expect(groupDigits('1000')).toBe('1,000');
    expect(groupDigits('0.123456')).toBe('0.123456');
  });

  it('leaves short numbers and zero unchanged', () => {
    expect(groupDigits('999')).toBe('999');
    expect(groupDigits('0')).toBe('0');
  });

  it('keeps the sign in front of the grouped digits', () => {
    expect(groupDigits('-1234567.891')).toBe('-1,234,567.891');
    expect(groupDigits('-100')).toBe('-100');
  });

  it('keeps every fractional digit, without rounding', () => {
    expect(groupDigits('2253433.8268696607')).toBe('2,253,433.8268696607');
  });
});

describe('formatGbp', () => {
  it('formats positive amounts', () => {
    expect(formatGbp('1234.50')).toBe('£1,234.50');
    expect(formatGbp('2253433.83')).toBe('£2,253,433.83');
  });

  it('puts the minus sign before the pound sign', () => {
    expect(formatGbp('-12.30')).toBe('-£12.30');
    expect(formatGbp('-24686.16')).toBe('-£24,686.16');
  });

  it('formats zero', () => {
    expect(formatGbp('0.00')).toBe('£0.00');
  });

  it('shows a dash for a missing amount', () => {
    expect(formatGbp(null)).toBe('—');
    expect(formatGbp(undefined)).toBe('—');
  });
});

describe('formatQuantity', () => {
  it('groups digits in whole quantities', () => {
    expect(formatQuantity(1726)).toBe('1,726');
    expect(formatQuantity(1234567)).toBe('1,234,567');
  });

  it('formats zero and negative quantities', () => {
    expect(formatQuantity(0)).toBe('0');
    expect(formatQuantity(-515)).toBe('-515');
  });

  it('shows a dash for a missing quantity', () => {
    expect(formatQuantity(null)).toBe('—');
    expect(formatQuantity(undefined)).toBe('—');
  });
});

describe('signClass', () => {
  it('classifies positive and negative amounts', () => {
    expect(signClass('8462.05')).toBe('positive');
    expect(signClass('-2200.79')).toBe('negative');
  });

  it('gives zero, in any form, no colour', () => {
    expect(signClass('0')).toBe('');
    expect(signClass('0.00')).toBe('');
    expect(signClass('-0.00')).toBe('');
  });

  it('gives a missing or empty amount no colour', () => {
    expect(signClass(null)).toBe('');
    expect(signClass(undefined)).toBe('');
    expect(signClass('')).toBe('');
  });

  it('treats a small non-zero amount as signed', () => {
    expect(signClass('0.01')).toBe('positive');
    expect(signClass('-0.01')).toBe('negative');
  });
});

describe('formatTimestamp', () => {
  it('shows a UTC timestamp as date and time, without the T and Z', () => {
    expect(formatTimestamp('2026-01-22T08:52:12Z')).toBe('2026-01-22 08:52:12');
  });

  it('does not convert to the local time zone', () => {
    expect(formatTimestamp('2026-03-27T23:59:59Z')).toBe('2026-03-27 23:59:59');
  });
});
