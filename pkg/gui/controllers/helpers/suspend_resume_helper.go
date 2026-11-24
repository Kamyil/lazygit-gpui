package helpers

type SuspendResumeHelper struct {
	c *HelperCommon

	// True if lazygit runs for the desktop frontend, which shares lazygit's
	// process group and so would be stopped along with it.
	inDesktopFrontend bool
}

func NewSuspendResumeHelper(c *HelperCommon, inDesktopFrontend bool) *SuspendResumeHelper {
	return &SuspendResumeHelper{
		c:                 c,
		inDesktopFrontend: inDesktopFrontend,
	}
}

// CannotSuspendAppReason returns why the app can't be suspended, or "" if it
// can.
func (s *SuspendResumeHelper) CannotSuspendAppReason() string {
	if s.inDesktopFrontend {
		return s.c.Tr.CannotSuspendDesktopApp
	}
	if !canSuspendApp() {
		return s.c.Tr.CannotSuspendApp
	}
	return ""
}

func (s *SuspendResumeHelper) SuspendApp() error {
	if s.CannotSuspendAppReason() != "" {
		return nil
	}

	if err := s.c.Suspend(); err != nil {
		return err
	}

	return sendStopSignal()
}

func (s *SuspendResumeHelper) InstallResumeSignalHandler() {
	installResumeSignalHandler(s.c.Log, s.c.Resume)
}
