import { useState } from 'react';

interface DepositButtonProps {
  matchId: number;
  disabled?: boolean;
  deposited?: boolean;
  onDeposit: (matchId: number) => Promise<void>;
}

export function DepositButton({
  matchId,
  disabled,
  deposited = false,
  onDeposit,
}: DepositButtonProps) {
  const [loading, setLoading] = useState(false);
  const [depositedLocally, setDepositedLocally] = useState(false);
  const [feedback, setFeedback] = useState<{ ok: boolean; msg: string } | null>(null);

  async function handleClick() {
    setLoading(true);
    setFeedback(null);
    try {
      await onDeposit(matchId);
      setDepositedLocally(true);
      setFeedback({ ok: true, msg: 'Deposit successful!' });
    } catch (e) {
      setFeedback({ ok: false, msg: e instanceof Error ? e.message : 'Deposit failed.' });
    } finally {
      setLoading(false);
    }
  }

  const alreadyDeposited = deposited || depositedLocally;

  return (
    <div>
      <button
        onClick={handleClick}
        disabled={disabled || loading || alreadyDeposited}
        aria-busy={loading}
      >
        {loading ? <span aria-label="loading">⏳</span> : alreadyDeposited ? 'Deposited' : 'Deposit'}
      </button>
      {feedback && (
        <p role="status" style={{ color: feedback.ok ? 'green' : 'red' }}>
          {feedback.msg}
        </p>
      )}
    </div>
  );
}
