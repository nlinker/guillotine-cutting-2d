'' =============================================================================
'' pi_solver.bas  —  VBA module for Excel
'' =============================================================================
''
'' How to use:
''   1. In Excel: Alt+F11 → Insert → Module → paste this code
''   2. Place pi_solver.exe in the same folder as the .xlsm file
''   3. Run macro RunSolver
''   4. Progress will update every second in the sheet cells
''
'' Cells (sheet "Sheet1"):
''   B1  — status ("Running..." / "Done!")
''   B2  — second
''   B3  — iterations completed
''   B4  — current PI estimate
''   B5  — error (|PI_est - PI|)
''   B6  — path to EXE (can be changed)
'' =============================================================================

Option Explicit

'' === Windows API =============================================================

Private Declare PtrSafe Function CreateFile Lib "kernel32" Alias "CreateFileW" ( _
    ByVal lpFileName        As LongPtr, _
    ByVal dwDesiredAccess   As Long, _
    ByVal dwShareMode       As Long, _
    ByVal lpSecurityAttrib  As LongPtr, _
    ByVal dwCreationDisp    As Long, _
    ByVal dwFlagsAndAttrib  As Long, _
    ByVal hTemplateFile     As LongPtr _
) As LongPtr

Private Declare PtrSafe Function ReadFile Lib "kernel32" ( _
    ByVal hFile                 As LongPtr, _
    ByVal lpBuffer              As LongPtr, _
    ByVal nNumberOfBytesToRead  As Long, _
    ByRef lpNumberOfBytesRead   As Long, _
    ByVal lpOverlapped          As LongPtr _
) As Long

Private Declare PtrSafe Function CloseHandle Lib "kernel32" ( _
    ByVal hObject As LongPtr _
) As Long

Private Declare PtrSafe Function WaitNamedPipe Lib "kernel32" Alias "WaitNamedPipeW" ( _
    ByVal lpNamedPipeName As LongPtr, _
    ByVal nTimeOut        As Long _
) As Long

Private Declare PtrSafe Sub Sleep Lib "kernel32" (ByVal dwMilliseconds As Long)

'' === Constants ===============================================================

Private Const PIPE_NAME         As String = "\\.\pipe\pi_solver_progress"
Private Const GENERIC_READ      As Long   = &H80000000
Private Const OPEN_EXISTING     As Long   = 3
Private Const FILE_ATTR_NORMAL  As Long   = &H80
Private Const INVALID_HANDLE    As Long   = -1
Private Const BUFFER_SIZE       As Long   = 4096

'' === Global variables ========================================================

Private g_Running As Boolean   ' True while calculation is active

'' === Helper: convert String to Unicode pointer for WinAPI ====================

Private Function StrToWPtr(s As String) As LongPtr
    StrToWPtr = StrPtr(s)
End Function

'' === Find EXE next to the workbook ===========================================

Private Function GetExePath() As String
    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets("Sheet1")

    ' If B6 already has a path, use it
    If ws.Range("B6").Value <> "" Then
        GetExePath = ws.Range("B6").Value
    Else
        ' Default: workbook folder
        GetExePath = ThisWorkbook.Path & "\pi_solver.exe"
        ws.Range("B6").Value = GetExePath
    End If
End Function

'' === Write status to cells ===================================================

Private Sub UpdateCells(status As String, secs As String, _
                        iters As String, piVal As String, errVal As String)
    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets("Sheet1")

    ws.Range("B1").Value = status
    If secs   <> "" Then ws.Range("B2").Value = CLng(secs)
    If iters  <> "" Then ws.Range("B3").Value = CDbl(iters)
    If piVal  <> "" Then ws.Range("B4").Value = CDbl(piVal)
    If errVal <> "" Then ws.Range("B5").Value = CDbl(errVal)

    DoEvents  ' repaint screen
End Sub

'' === Initialise sheet headers ================================================

Private Sub InitSheet()
    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets("Sheet1")

    ws.Range("A1").Value = "Status"
    ws.Range("A2").Value = "Time (sec)"
    ws.Range("A3").Value = "Iterations"
    ws.Range("A4").Value = "PI estimate"
    ws.Range("A5").Value = "Error"
    ws.Range("A6").Value = "Path to EXE"

    ws.Range("B4").NumberFormat = "0.0000000000"
    ws.Range("B5").NumberFormat = "0.00E+00"

    ws.Columns("A").AutoFit
End Sub

'' === Main macro ==============================================================

Public Sub RunSolver()
    If g_Running Then
        MsgBox "Calculation is already running!", vbInformation
        Exit Sub
    End If

    InitSheet

    Dim exePath As String
    exePath = GetExePath()

    If Dir(exePath) = "" Then
        MsgBox "File not found: " & exePath & Chr(13) & _
               "Set the correct path in cell B6.", vbCritical
        Exit Sub
    End If

    '' == Launch EXE =======================================================
    UpdateCells "Starting...", "", "", "", ""
    Shell Chr(34) & exePath & Chr(34), vbHide

    '' == Brief pause to let EXE create the pipe ===========================
    Sleep 500

    '' == Connect to the pipe (with retries) ===============================
    Dim pipeName As String
    pipeName = PIPE_NAME

    Dim hPipe As LongPtr
    Dim attempt As Integer

    For attempt = 1 To 10
        ' Wait until the pipe becomes available (up to 2s per attempt)
        WaitNamedPipe StrToWPtr(pipeName & Chr(0)), 2000

        hPipe = CreateFile( _
            StrToWPtr(pipeName & Chr(0)), _
            GENERIC_READ, _
            0, _
            0, _
            OPEN_EXISTING, _
            FILE_ATTR_NORMAL, _
            0 _
        )

        If hPipe <> INVALID_HANDLE Then Exit For

        Sleep 500
        DoEvents
    Next attempt

    If hPipe = INVALID_HANDLE Then
        MsgBox "Failed to connect to the pipe." & Chr(13) & _
               "Make sure pi_solver.exe started successfully.", vbCritical
        UpdateCells "Connection error", "", "", "", ""
        Exit Sub
    End If

    '' == Read messages from the pipe ======================================
    g_Running = True
    UpdateCells "Running...", "", "", "", ""

    Dim buffer()    As Byte
    Dim bytesRead   As Long
    Dim rawMsg      As String
    Dim leftover    As String  ' remainder of an incomplete line

    ReDim buffer(BUFFER_SIZE - 1)
    leftover = ""

    Do While g_Running
        bytesRead = 0
        Dim ok As Long
        ok = ReadFile(hPipe, VarPtr(buffer(0)), BUFFER_SIZE, bytesRead, 0)

        If ok = 0 Or bytesRead = 0 Then
            ' Pipe closed — EXE finished
            Exit Do
        End If

        '' Convert bytes (UTF-8) to VBA string
        '' Messages are ASCII-compatible so Chr() is fine
        Dim i As Long
        rawMsg = leftover
        For i = 0 To bytesRead - 1
            rawMsg = rawMsg & Chr(buffer(i))
        Next i
        leftover = ""

        '' Split into lines (delimiter \n)
        Dim lines() As String
        lines = Split(rawMsg, Chr(10))

        Dim lineIdx As Integer
        For lineIdx = 0 To UBound(lines)
            Dim line As String
            line = Trim(lines(lineIdx))

            If line = "" Then
                ' empty line — skip
            ElseIf Left(line, 9) = "PROGRESS|" Then
                '' PROGRESS|<sec>|<iter>|<pi>|<err>
                Dim parts() As String
                parts = Split(line, "|")
                If UBound(parts) >= 4 Then
                    UpdateCells "Running...", parts(1), parts(2), parts(3), parts(4)
                End If

            ElseIf Left(line, 5) = "DONE|" Then
                '' DONE|<iter>|<pi>
                Dim dparts() As String
                dparts = Split(line, "|")
                If UBound(dparts) >= 2 Then
                    Dim finalPi As Double
                    finalPi = CDbl(dparts(2))
                    Dim finalErr As Double
                    finalErr = Abs(finalPi - 3.14159265358979)
                    UpdateCells "Done! ✓", "", dparts(1), dparts(2), _
                                Format(finalErr, "0.00E+00")
                End If
                g_Running = False

            ElseIf Left(line, 6) = "ERROR|" Then
                UpdateCells "Error: " & Mid(line, 7), "", "", "", ""
                g_Running = False

            Else
                ' Incomplete line (last Split element without \n)
                If lineIdx = UBound(lines) Then
                    leftover = line
                End If
            End If
        Next lineIdx

        DoEvents
        Sleep 100  ' brief pause between reads

    Loop

    '' == Close the pipe ===================================================
    CloseHandle hPipe
    g_Running = False

    If ThisWorkbook.Sheets("Sheet1").Range("B1").Value <> "Done! ✓" Then
        UpdateCells "Finished", "", "", "", ""
    End If

    MsgBox "Calculation complete! PI ≈ " & ThisWorkbook.Sheets("Sheet1").Range("B4").Value, _
           vbInformation, "pi_solver"
End Sub

'' === Stop calculation early ==================================================

Public Sub StopSolver()
    g_Running = False
    ThisWorkbook.Sheets("Sheet1").Range("B1").Value = "Stopped"
End Sub
