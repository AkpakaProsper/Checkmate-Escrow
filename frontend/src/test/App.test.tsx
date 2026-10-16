import { describe, it, expect, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import App from '../App';
import * as walletHooks from '../hooks/useWallet';

// Stub wallet adapters so the hook doesn't hit real browser APIs.
vi.mock('../wallets/freighter', () => ({
  freighterIsAvailable: vi.fn().mockResolvedValue(false),
  freighterGetPublicKey: vi.fn(),
  freighterSign: vi.fn(),
}));
vi.mock('../wallets/albedo', () => ({
  albedoIsAvailable: vi.fn().mockReturnValue(false),
  albedoGetPublicKey: vi.fn(),
  albedoSign: vi.fn(),
}));

describe('App — landing page', () => {
  it('renders the ThemeToggle button on the landing page', () => {
    render(<App />);
    // ThemeToggle renders a button with an aria-label about switching modes.
    const toggleButton = screen.getByRole('button', {
      name: /switch to (light|dark) mode/i,
    });
    expect(toggleButton).toBeDefined();
  });
});

describe('App — create match form', () => {
  it('resets form state when the connected wallet account changes', () => {
    const originalPath = window.location.pathname;
    window.history.pushState({}, '', '/create');
    const wallet = {
      type: 'freighter' as const,
      publicKey: 'GACCOUNTONE',
      connected: true,
      error: null,
      connect: vi.fn(),
      disconnect: vi.fn(),
    };
    const useWalletSpy = vi.spyOn(walletHooks, 'useWallet').mockReturnValue(wallet);

    try {
      const { rerender } = render(<App />);

      fireEvent.change(screen.getByLabelText(/player 2 address/i), {
        target: { value: 'GOLDACCOUNT' },
      });
      expect(screen.getByLabelText(/player 2 address/i)).toHaveValue('GOLDACCOUNT');

      useWalletSpy.mockReturnValue({ ...wallet, publicKey: 'GACCOUNTTWO' });
      rerender(<App />);

      expect(screen.getByLabelText(/player 2 address/i)).toHaveValue('');
    } finally {
      useWalletSpy.mockRestore();
      window.history.pushState({}, '', originalPath);
    }
  });
});
