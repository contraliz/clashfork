import {
  useEffect,
  useId,
  useRef,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'
import { Icon, type IconName } from './Icon'

type ButtonVariant = 'primary' | 'secondary' | 'quiet' | 'danger'

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant
  busy?: boolean
  icon?: IconName
}

export function Button({
  children,
  variant = 'secondary',
  busy = false,
  icon,
  className = '',
  disabled,
  ...props
}: ButtonProps) {
  return (
    <button
      type="button"
      className={`button button-${variant} ${className}`.trim()}
      disabled={disabled || busy}
      aria-busy={busy || undefined}
      {...props}
    >
      {busy ? <span className="spinner" aria-hidden="true" /> : icon ? <Icon name={icon} size={17} /> : null}
      {children}
    </button>
  )
}

export function IconButton({
  icon,
  label,
  variant = 'quiet',
  className = '',
  ...props
}: { icon: IconName; label: string; variant?: ButtonVariant } & ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      type="button"
      className={`icon-button icon-button-${variant} ${className}`.trim()}
      aria-label={label}
      title={label}
      {...props}
    >
      <Icon name={icon} size={18} />
    </button>
  )
}

export function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean
  onChange: (checked: boolean) => void
  label: string
  disabled?: boolean
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      className={`toggle ${checked ? 'is-on' : ''}`}
      onClick={() => onChange(!checked)}
      disabled={disabled}
    >
      <span className="toggle-knob" />
    </button>
  )
}

export function Progress({ value, label }: { value: number; label: string }) {
  const clamped = Math.min(100, Math.max(0, value))
  return (
    <div className="progress" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(clamped)}>
      <span style={{ width: `${clamped}%` }} />
    </div>
  )
}

export function Panel({
  children,
  className = '',
  ...props
}: { children: ReactNode; className?: string } & HTMLAttributes<HTMLElement>) {
  return <section className={`panel ${className}`.trim()} {...props}>{children}</section>
}

export function PanelHeading({
  title,
  id,
  kicker,
  description,
  actions,
  className = '',
}: {
  title: ReactNode
  id?: string
  kicker?: string
  description?: ReactNode
  actions?: ReactNode
  className?: string
}) {
  return (
    <div className={`panel-heading ${className}`.trim()}>
      <div className="panel-heading-copy">
        {kicker ? <div className="panel-kicker">{kicker}</div> : null}
        <h2 id={id}>{title}</h2>
        {description ? <p className="panel-description">{description}</p> : null}
      </div>
      {actions ? <div className="panel-heading-actions">{actions}</div> : null}
    </div>
  )
}

export function Grid({ children, className = '' }: { children: ReactNode; className?: string }) {
  return <div className={`ui-grid ${className}`.trim()}>{children}</div>
}

export function GridItem({
  children,
  span = 12,
  className = '',
}: {
  children: ReactNode
  span?: 3 | 4 | 5 | 6 | 7 | 8 | 9 | 12
  className?: string
}) {
  return <div className={`ui-grid-item span-${span} ${className}`.trim()}>{children}</div>
}

export function Toolbar({
  children,
  className = '',
  ...props
}: { children: ReactNode; className?: string } & HTMLAttributes<HTMLDivElement>) {
  return <div className={`toolbar ${className}`.trim()} {...props}>{children}</div>
}

export function EmptyState({
  title,
  description,
  action,
}: {
  title: string
  description?: string
  action?: ReactNode
}) {
  return (
    <div className="empty-state">
      <div className="empty-mark" aria-hidden="true" />
      <h3>{title}</h3>
      {description ? <p>{description}</p> : null}
      {action}
    </div>
  )
}

export function Field({
  label,
  hint,
  children,
  className = '',
}: {
  label: string
  hint?: string
  children: ReactNode
  className?: string
}) {
  return (
    <label className={`field ${className}`.trim()}>
      <span className="field-label">{label}</span>
      {children}
      {hint ? <span className="field-hint">{hint}</span> : null}
    </label>
  )
}

export function Banner({
  tone = 'error',
  children,
  onDismiss,
}: {
  tone?: 'error' | 'info' | 'warning'
  children: ReactNode
  onDismiss?: () => void
}) {
  return (
    <div className={`banner banner-${tone}`} role={tone === 'error' ? 'alert' : 'status'}>
      <Icon name={tone === 'error' || tone === 'warning' ? 'alert' : 'check'} size={18} />
      <div className="banner-copy">{children}</div>
      {onDismiss ? <IconButton icon="close" label="Dismiss message" onClick={onDismiss} /> : null}
    </div>
  )
}

export function Modal({
  title,
  description,
  children,
  footer,
  onClose,
  wide = false,
}: {
  title: string
  description?: string
  children: ReactNode
  footer?: ReactNode
  onClose: () => void
  wide?: boolean
}) {
  const titleId = useId()
  const dialogRef = useRef<HTMLDivElement>(null)
  const previousFocus = useRef<Element | null>(null)

  useEffect(() => {
    previousFocus.current = document.activeElement
    const previousOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    const frame = requestAnimationFrame(() => {
      const dialog = dialogRef.current
      if (!dialog || dialog.contains(document.activeElement)) return
      const auto = dialog.querySelector<HTMLElement>('[data-autofocus]')
      ;(auto ?? dialog).focus()
    })
    return () => {
      cancelAnimationFrame(frame)
      document.body.style.overflow = previousOverflow
      if (previousFocus.current instanceof HTMLElement) previousFocus.current.focus()
    }
  }, [])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
      if (event.key !== 'Tab' || !dialogRef.current) return
      const focusable = dialogRef.current.querySelectorAll<HTMLElement>(
        'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      )
      if (focusable.length === 0) {
        event.preventDefault()
        return
      }
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [onClose])

  return createPortal(
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
    >
      <div
        ref={dialogRef}
        className={`modal ${wide ? 'modal-wide' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
      >
        <header className="modal-head">
          <div>
            <h2 id={titleId}>{title}</h2>
            {description ? <p>{description}</p> : null}
          </div>
          <IconButton icon="close" label="Close dialog" onClick={onClose} />
        </header>
        <div className="modal-body">{children}</div>
        {footer ? <footer className="modal-footer">{footer}</footer> : null}
      </div>
    </div>,
    document.body,
  )
}

export function PageHeader({
  title,
  actions,
  eyebrow,
}: {
  title: string
  actions?: ReactNode
  eyebrow?: string
}) {
  return (
    <header className="page-header">
      <div>
        {eyebrow ? <div className="page-eyebrow">{eyebrow}</div> : null}
        <h1>{title}</h1>
      </div>
      {actions ? <div className="page-actions">{actions}</div> : null}
    </header>
  )
}

export function SourceBadge({
  children,
  tone = 'neutral',
  dot = false,
}: {
  children: ReactNode
  tone?: 'neutral' | 'accent' | 'violet' | 'amber' | 'red'
  dot?: boolean
}) {
  return (
    <span className={`badge badge-${tone}`}>
      {dot ? <span className="badge-dot" aria-hidden="true" /> : null}
      {children}
    </span>
  )
}
